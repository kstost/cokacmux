//! One settings writer for all UI changes. A slow disk retains at most one
//! pending snapshot, and an older drag cannot overwrite a newer keyboard edit.
use super::*;

type SaveJob = (u64, Settings);

pub(super) struct SettingsWriter {
    wake: SyncSender<()>,
    pending: Arc<Mutex<Option<SaveJob>>>,
    completed: Receiver<std::result::Result<(), String>>,
    seq: u64,
    pub(super) saving: bool,
}

impl SettingsWriter {
    fn start(main_tx: Sender<MainEvent>) -> io::Result<Self> {
        Self::start_with(main_tx, |settings| {
            settings.save().map_err(|error| error.to_string())
        })
    }

    fn start_with(
        main_tx: Sender<MainEvent>,
        mut save: impl FnMut(Settings) -> std::result::Result<(), String> + Send + 'static,
    ) -> io::Result<Self> {
        let (wake, wake_rx) = mpsc::sync_channel(1);
        let pending = Arc::new(Mutex::new(None::<SaveJob>));
        let worker_pending = Arc::clone(&pending);
        let (done_tx, completed) = mpsc::channel();
        thread::Builder::new()
            .name("cokacmux-settings-writer".into())
            .spawn(move || {
                let mut last_result = Ok(());
                loop {
                    // Never hold the pending lock during filesystem work.
                    let job = worker_pending
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .take();
                    if let Some((seq, settings)) = job {
                        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                            save(settings)
                        }))
                        .map_err(|_| "settings writer panicked".to_string())
                        .and_then(|result| result);
                        last_result = result.clone();
                        let _ = main_tx.send(MainEvent::SettingsSaved { seq, result });
                    } else if wake_rx.recv().is_err() {
                        break;
                    }
                }
                let _ = done_tx.send(last_result);
            })?;
        Ok(Self {
            wake,
            pending,
            completed,
            seq: 0,
            saving: false,
        })
    }

    fn queue(&mut self, settings: Settings) -> io::Result<()> {
        self.seq += 1;
        self.pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .replace((self.seq, settings));
        match self.wake.try_send(()) {
            Ok(()) | Err(mpsc::TrySendError::Full(())) => {
                self.saving = true;
                Ok(())
            }
            Err(mpsc::TrySendError::Disconnected(())) => {
                Err(io::Error::other("settings writer disconnected"))
            }
        }
    }

    /// Called after restoring the terminal, including on a TUI error. A normal
    /// exit must not abandon a final width just because the disk is slow.
    pub(super) fn finish(self) -> std::result::Result<(), String> {
        drop(self.wake);
        self.completed
            .recv()
            .map_err(|_| "settings writer disconnected".to_string())?
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stalled_save_coalesces_width_and_later_settings_edits_in_order() {
        let (tx, rx) = mpsc::channel();
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let mut first = true;
        let mut writer = SettingsWriter::start_with(tx, move |settings| {
            started_tx.send(settings).unwrap();
            if first {
                first = false;
                release_rx.recv_timeout(Duration::from_secs(5)).unwrap();
            }
            Ok(())
        })
        .unwrap();
        let mut settings = Settings::default();
        settings.cokacmux.sessions_pane_width = Some(41);
        writer.queue(settings.clone()).unwrap();
        assert_eq!(
            started_rx
                .recv_timeout(Duration::from_secs(5))
                .unwrap()
                .cokacmux
                .sessions_pane_width,
            Some(41)
        );
        // These submissions complete while the worker is blocked in save().
        settings.cokacmux.sessions_pane_width = Some(42);
        writer.queue(settings.clone()).unwrap();
        settings.cokacmux.sessions_pane_width = Some(43);
        settings.cokacmux.agent_sidebar_visible = false;
        writer.queue(settings).unwrap();
        release_tx.send(()).unwrap();
        let latest = started_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(latest.cokacmux.sessions_pane_width, Some(43));
        assert!(!latest.cokacmux.agent_sidebar_visible);
        writer.finish().unwrap();
        assert!(started_rx.try_recv().is_err());
        let results: Vec<_> = rx
            .try_iter()
            .filter_map(|event| match event {
                MainEvent::SettingsSaved { seq, result } => {
                    assert!(result.is_ok());
                    Some(seq)
                }
                _ => None,
            })
            .collect();
        assert_eq!(results, [1, 3]);
    }

    #[test]
    fn save_failure_is_reported_and_a_later_save_can_succeed() {
        let (tx, rx) = mpsc::channel();
        let mut first = true;
        let mut writer = SettingsWriter::start_with(tx, move |_| {
            if std::mem::take(&mut first) {
                Err("disk unavailable".into())
            } else {
                Ok(())
            }
        })
        .unwrap();
        writer.queue(Settings::default()).unwrap();
        assert!(matches!(
            rx.recv_timeout(Duration::from_secs(5)).unwrap(),
            MainEvent::SettingsSaved {
                seq: 1,
                result: Err(_)
            }
        ));
        writer.queue(Settings::default()).unwrap();
        writer.finish().unwrap();
        assert!(matches!(
            rx.recv_timeout(Duration::from_secs(5)).unwrap(),
            MainEvent::SettingsSaved {
                seq: 2,
                result: Ok(())
            }
        ));
    }

    #[test]
    fn shutdown_reports_an_unsaved_final_change() {
        let (tx, _) = mpsc::channel();
        let mut writer =
            SettingsWriter::start_with(tx, |_| Err("disk unavailable".into())).unwrap();
        writer.queue(Settings::default()).unwrap();
        assert_eq!(writer.finish().unwrap_err(), "disk unavailable");
    }

    #[test]
    fn shutdown_drains_latest_settings_and_preserves_unknown_fields() {
        let directory =
            std::env::temp_dir().join(format!("resize-settings-{}", uuid::Uuid::now_v7()));
        let path = directory.join("settings.json");
        let worker_path = path.clone();
        let (tx, _) = mpsc::channel();
        let mut writer = SettingsWriter::start_with(tx, move |settings| {
            settings
                .save_to_path(&worker_path)
                .map_err(|e| e.to_string())
        })
        .unwrap();
        let mut settings = Settings::default();
        settings.cokacmux.sessions_pane_width = Some(67);
        settings.cokacmux.agent_sidebar_width = 31;
        settings.cokacmux.agent_aux_width = Some(54);
        settings
            .extra
            .insert("user_note".into(), serde_json::json!("keep"));
        writer.queue(settings).unwrap();
        writer.finish().unwrap();
        let saved: Settings = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(saved.cokacmux.sessions_pane_width, Some(67));
        assert_eq!(saved.cokacmux.agent_sidebar_width, 31);
        assert_eq!(saved.cokacmux.agent_aux_width, Some(54));
        assert_eq!(saved.extra["user_note"], "keep");
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn stale_completion_does_not_report_latest_edit_saved_or_failed() {
        let mut app = super::super::tests::app_for_key_tests();
        let (wake, _wake_rx) = mpsc::sync_channel(1);
        let (_, completed) = mpsc::channel();
        app.settings_writer = Some(SettingsWriter {
            wake,
            pending: Arc::new(Mutex::new(None)),
            completed,
            seq: 2,
            saving: true,
        });
        app.status = "layout save pending.".into();
        app.settings_saved(1, Err("old failure".into()));
        assert_eq!(app.status, "layout save pending.");
        app.settings_saved(2, Ok(()));
        assert_eq!(app.status, "layout saved.");
    }
}

impl App {
    pub(super) fn save_settings(&mut self) -> Result<()> {
        #[cfg(test)]
        if self.settings.skip_save {
            return Ok(());
        }
        if self.settings_writer.is_none() {
            let tx = self
                .main_tx
                .clone()
                .ok_or_else(|| anyhow::anyhow!("settings event channel unavailable"))?;
            self.settings_writer = Some(SettingsWriter::start(tx)?);
        }
        self.settings_writer
            .as_mut()
            .unwrap()
            .queue(self.settings.clone())?;
        Ok(())
    }

    pub(super) fn settings_save_word(&self) -> &'static str {
        if self
            .settings_writer
            .as_ref()
            .is_some_and(|writer| writer.saving)
        {
            "save pending"
        } else {
            "saved"
        }
    }

    pub(super) fn settings_saved(&mut self, seq: u64, result: std::result::Result<(), String>) {
        let Some(writer) = self.settings_writer.as_mut() else {
            return;
        };
        if seq != writer.seq {
            return;
        }
        writer.saving = false;
        match result {
            Ok(()) => {
                if self.status.contains("save pending") {
                    self.status = self.status.replacen("save pending", "saved", 1);
                }
            }
            Err(error) => self.status = format!("settings changed, save failed: {error}"),
        }
    }
}
