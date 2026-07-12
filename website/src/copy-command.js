export async function copyText(
  text,
  navigatorObject = globalThis.navigator,
  documentObject = globalThis.document
) {
  if (navigatorObject?.clipboard?.writeText) {
    try {
      await navigatorObject.clipboard.writeText(text);
      return;
    } catch {
      // Clipboard permissions can reject even when the API exists.  The
      // selection-based fallback still works in some of those environments.
    }
  }

  if (!documentObject?.body || typeof documentObject.createElement !== 'function') {
    throw new Error('no clipboard implementation is available');
  }

  const textArea = documentObject.createElement('textarea');
  textArea.value = text;
  textArea.setAttribute('readonly', '');
  textArea.style.position = 'fixed';
  textArea.style.opacity = '0';
  documentObject.body.appendChild(textArea);
  try {
    textArea.select();
    if (
      typeof documentObject.execCommand !== 'function' ||
      !documentObject.execCommand('copy')
    ) {
      throw new Error('copy command was rejected');
    }
  } finally {
    textArea.remove();
  }
}
