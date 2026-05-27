"""
Tool installation and management for cross-compilation.
Installs Rust, zig, cargo-zigbuild, and macOS SDK into the builder/tools directory.
"""
import os
import shutil
import subprocess
import struct
import tarfile
import zipfile
from pathlib import Path
from typing import Dict, List, Optional
import urllib.request
import ssl

from .config import BuildConfig
from .logger import Logger


WINDOWS_IMPORT_DLLS: Dict[str, str] = {
    "advapi32": "advapi32.dll",
    "cfgmgr32": "cfgmgr32.dll",
    "gdi32": "gdi32.dll",
    "msimg32": "msimg32.dll",
    "ole32": "ole32.dll",
    "oleaut32": "oleaut32.dll",
    "opengl32": "opengl32.dll",
    "winspool": "winspool.drv",
}

WINDOWS_STUB_IMPORT_DLLS: Dict[str, str] = {
    # The MinGW target requests -lsynchronization, while modern Windows exposes
    # the synchronization entry points through API-set DLLs. An empty import
    # archive is enough because this project does not reference those symbols.
    "synchronization": "api-ms-win-core-synch-l1-2-0.dll",
}


class ToolInstaller:
    """Manages installation of build tools."""

    def __init__(self, config: BuildConfig, project_root: Path, logger: Logger):
        self.config = config
        self.project_root = project_root
        self.tools_dir = project_root / config.tools_dir
        self.logger = logger

        # Rust directories (local installation)
        self.cargo_home = self.tools_dir / "cargo"
        self.rustup_home = self.tools_dir / "rustup"

        # Specific tool directories
        self.zig_dir = self.tools_dir / f"zig-{config.zig_version}"
        self.sdk_dir = self.tools_dir / f"MacOSX{config.macos_sdk_version}.sdk"
        self.winlibs_root = self.tools_dir / "winlibs"

    def _exe_name(self, name: str) -> str:
        """Return the platform-specific executable file name."""
        return f"{name}.exe" if os.name == "nt" else name

    def _path_env_key(self, env: dict) -> str:
        """Return the existing PATH key, preserving Windows' Path casing."""
        for key in env:
            if key.upper() == "PATH":
                return key
        return "PATH"

    def _resolve_command(self, name: str, env: Optional[dict] = None) -> str:
        """Resolve a command through PATH and return an executable path if found."""
        env = env or os.environ
        path_value = env.get(self._path_env_key(env), None)
        candidates = [self._exe_name(name)]
        if candidates[0] != name:
            candidates.append(name)
        for candidate in candidates:
            resolved = shutil.which(candidate, path=path_value)
            if resolved:
                return resolved
        return name

    def ensure_tools_dir(self) -> None:
        """Create tools directory if it doesn't exist."""
        self.tools_dir.mkdir(parents=True, exist_ok=True)

    # ==================== Rust Installation ====================

    def get_cargo_path(self) -> Optional[Path]:
        """Get path to cargo executable."""
        # Check local installation first
        local_cargo = self.cargo_home / "bin" / self._exe_name("cargo")
        if local_cargo.exists():
            return local_cargo

        # Check system installation
        system_cargo = shutil.which("cargo")
        if system_cargo:
            return Path(system_cargo)

        return None

    def get_rustup_path(self) -> Optional[Path]:
        """Get path to rustup executable."""
        # Check local installation first
        local_rustup = self.cargo_home / "bin" / self._exe_name("rustup")
        if local_rustup.exists():
            return local_rustup

        # Check system installation
        system_rustup = shutil.which("rustup")
        if system_rustup:
            return Path(system_rustup)

        return None

    def is_rust_installed(self) -> bool:
        """Check if Rust is installed."""
        return self.get_cargo_path() is not None and self.get_rustup_path() is not None

    def _get_rust_env(self) -> dict:
        """Get environment variables for Rust operations."""
        env = os.environ.copy()
        env["CARGO_HOME"] = str(self.cargo_home)
        env["RUSTUP_HOME"] = str(self.rustup_home)

        # Add cargo bin to PATH
        cargo_bin = self.cargo_home / "bin"
        path_key = self._path_env_key(env)
        current_path = env.get(path_key, "")
        env[path_key] = f"{cargo_bin}{os.pathsep}{current_path}"

        return env

    def install_rust(self) -> bool:
        """Install Rust toolchain into builder/tools directory."""
        if self.is_rust_installed():
            cargo_path = self.get_cargo_path()
            self.logger.success(f"Rust is already installed at {cargo_path}")
            return True

        self.ensure_tools_dir()
        self.logger.info("Installing Rust toolchain...")

        # Download rustup-init
        if self.config.host_os == "windows":
            rust_target = f"{self.config.host_arch}-pc-windows-msvc"
            rustup_init_url = (
                "https://static.rust-lang.org/rustup/dist/"
                f"{rust_target}/rustup-init.exe"
            )
            rustup_init_path = self.tools_dir / "rustup-init.exe"
        else:
            rustup_init_url = "https://sh.rustup.rs"
            rustup_init_path = self.tools_dir / "rustup-init.sh"

        try:
            self.logger.info("Downloading rustup installer...")
            ctx = ssl.create_default_context()

            with urllib.request.urlopen(rustup_init_url, context=ctx) as response:
                script_content = response.read()
                with open(rustup_init_path, "wb") as f:
                    f.write(script_content)

            if os.name != "nt":
                rustup_init_path.chmod(0o755)

            # Prepare environment for installation
            env = self._get_rust_env()

            # Run rustup-init with options:
            # -y: don't prompt
            # --no-modify-path: don't modify shell profiles
            # --default-toolchain stable: install stable toolchain
            self.logger.info("Running rustup installer (this may take a while)...")

            result = subprocess.run(
                [
                    str(rustup_init_path),
                    "-y",
                    "--no-modify-path",
                    "--default-toolchain", "stable",
                ],
                env=env,
                capture_output=True,
                text=True,
            )

            if result.returncode == 0:
                self.logger.success(f"Rust installed at {self.cargo_home}")

                # Verify installation
                cargo_path = self.cargo_home / "bin" / self._exe_name("cargo")
                if cargo_path.exists():
                    # Get version
                    version_result = subprocess.run(
                        [str(cargo_path), "--version"],
                        capture_output=True,
                        text=True,
                        env=env,
                    )
                    if version_result.returncode == 0:
                        self.logger.info(f"  {version_result.stdout.strip()}")

                return True
            else:
                self.logger.error(f"Rust installation failed: {result.stderr}")
                return False

        except Exception as e:
            self.logger.error(f"Failed to install Rust: {e}")
            return False

        finally:
            # Cleanup installer
            if rustup_init_path.exists():
                rustup_init_path.unlink()

    # ==================== Zig Installation ====================

    def get_zig_path(self) -> Optional[Path]:
        """Get path to zig executable."""
        zig_exe = self.zig_dir / self._exe_name("zig")
        if zig_exe.exists():
            return zig_exe

        # Check if zig is in system PATH
        system_zig = shutil.which("zig")
        if system_zig:
            return Path(system_zig)

        return None

    def is_zig_installed(self) -> bool:
        """Check if zig is installed."""
        return self.get_zig_path() is not None

    def install_zig(self) -> bool:
        """Install zig compiler."""
        if self.is_zig_installed():
            zig_path = self.get_zig_path()
            self.logger.success(f"Zig is already installed at {zig_path}")
            return True

        self.ensure_tools_dir()

        # Download zig
        archive_ext = "zip" if self.config.host_os == "windows" else "tar.xz"
        archive_name = (
            f"zig-{self.config.host_os}-{self.config.host_arch}-"
            f"{self.config.zig_version}.{archive_ext}"
        )
        archive_path = self.tools_dir / archive_name

        if not archive_path.exists():
            local_cache = Path.home() / ".rustbuilder" / archive_name
            if local_cache.exists():
                self.logger.info(f"Using cached {archive_name} from {local_cache.parent}")
                shutil.copy2(local_cache, archive_path)
            elif not self.download_file(self.config.zig_url, archive_path, "Zig compiler"):
                return False

        # Extract to tools directory
        if archive_ext == "zip":
            if not self.extract_zip(archive_path, self.tools_dir):
                return False
        else:
            if not self.extract_tar_xz(archive_path, self.tools_dir):
                return False

        # Rename to standard directory name
        extracted_dir = self.tools_dir / f"zig-{self.config.host_os}-{self.config.host_arch}-{self.config.zig_version}"
        if extracted_dir.exists() and extracted_dir != self.zig_dir:
            if self.zig_dir.exists():
                # Security: Verify the path is within tools_dir before deletion
                if not self._is_safe_path_for_deletion(self.zig_dir):
                    self.logger.error(f"Refusing to delete unsafe path: {self.zig_dir}")
                    return False
                shutil.rmtree(self.zig_dir)
            extracted_dir.rename(self.zig_dir)

        # Verify installation
        zig_exe = self.zig_dir / self._exe_name("zig")
        if zig_exe.exists():
            if os.name != "nt":
                zig_exe.chmod(0o755)
            self.logger.success(f"Zig installed at {self.zig_dir}")
            return True
        else:
            self.logger.error("Zig installation failed - executable not found")
            return False

    # ==================== Windows GNU/LLVM Support ====================

    def _windows_gnullvm_triples(self) -> List[str]:
        """Return Windows GNU/LLVM triples supported by this build helper."""
        return [
            "aarch64-pc-windows-gnullvm",
            "x86_64-pc-windows-gnullvm",
        ]

    def _windows_host_gnullvm_triple(self) -> str:
        """Return the GNU/LLVM triple matching the current Windows host arch."""
        return f"{self.config.host_arch}-pc-windows-gnullvm"

    def windows_import_lib_dir(self, rust_target: Optional[str] = None) -> Path:
        """Return the local import-library directory for a Windows GNU/LLVM target."""
        target = rust_target or self._windows_host_gnullvm_triple()
        return self.winlibs_root / target

    def windows_cc_wrapper_path(self, rust_target: str) -> Optional[Path]:
        """Return the Zig cc wrapper path for a Windows GNU/LLVM target."""
        if rust_target.startswith("aarch64-"):
            return self.project_root / "builder" / "zig-cc-aarch64-windows.cmd"
        if rust_target.startswith("x86_64-"):
            return self.project_root / "builder" / "zig-cc-x86_64-windows.cmd"
        return None

    def windows_ar_wrapper_path(self) -> Path:
        """Return the Zig ar wrapper path for Windows GNU/LLVM targets."""
        return self.project_root / "builder" / "zig-ar.cmd"

    def is_rust_toolchain_installed(self, toolchain: str) -> bool:
        """Check whether a rustup toolchain is installed."""
        rustup_path = self.get_rustup_path()
        if not rustup_path:
            return False
        try:
            result = subprocess.run(
                [str(rustup_path), "toolchain", "list"],
                capture_output=True,
                text=True,
                env=self.get_env(),
            )
            if result.returncode != 0:
                return False
            return any(line.split()[0] == toolchain for line in result.stdout.splitlines())
        except FileNotFoundError:
            return False

    def install_rust_toolchain(self, toolchain: str) -> bool:
        """Install a rustup toolchain if it is missing."""
        if self.is_rust_toolchain_installed(toolchain):
            self.logger.debug(f"Toolchain {toolchain} is already installed")
            return True

        rustup_path = self.get_rustup_path()
        if not rustup_path:
            self.logger.error("rustup not found. Please run --setup first.")
            return False

        self.logger.info(f"Installing Rust toolchain: {toolchain}")
        cmd = [str(rustup_path), "toolchain", "install", toolchain]
        if (
            self.config.host_os == "windows" and
            toolchain.endswith("pc-windows-gnullvm") and
            self.config.host_arch not in toolchain
        ):
            cmd.append("--force-non-host")
        try:
            result = subprocess.run(
                cmd,
                capture_output=True,
                text=True,
                env=self.get_env(),
            )
            if result.returncode == 0:
                self.logger.success(f"Toolchain {toolchain} installed")
                return True
            self.logger.error(f"Failed to install toolchain {toolchain}: {result.stderr}")
            return False
        except FileNotFoundError:
            self.logger.error("rustup not found. Please run --setup first.")
            return False

    def is_windows_import_libs_installed(self, rust_target: Optional[str] = None) -> bool:
        """Check whether generated Windows import libraries are available."""
        lib_dir = self.windows_import_lib_dir(rust_target)
        expected = list(WINDOWS_IMPORT_DLLS) + list(WINDOWS_STUB_IMPORT_DLLS)
        return all((lib_dir / f"lib{name}.a").exists() for name in expected)

    def install_windows_import_libs(self, rust_target: Optional[str] = None) -> bool:
        """
        Generate GNU import archives from the local Windows DLL export tables.

        Rust's Windows GNU/LLVM toolchains link with lib*.a archives. They are
        not shipped with stock rustup toolchains on Windows, so we generate the
        small set this project needs from System32 DLL exports using Zig's
        dlltool.
        """
        if self.config.host_os != "windows":
            self.logger.info("Windows import libraries are only generated on Windows")
            return True

        target = rust_target or self._windows_host_gnullvm_triple()
        if not target.endswith("pc-windows-gnullvm"):
            return True

        if self.is_windows_import_libs_installed(target):
            self.logger.success(f"Windows import libraries are ready for {target}")
            return True

        if not self.install_zig():
            return False

        zig_path = self.get_zig_path()
        if not zig_path:
            self.logger.error("Zig is required to generate Windows import libraries")
            return False

        machine = self._dlltool_machine(target)
        lib_dir = self.windows_import_lib_dir(target)
        lib_dir.mkdir(parents=True, exist_ok=True)

        success = True
        for lib_name, dll_name in WINDOWS_IMPORT_DLLS.items():
            lib_path = lib_dir / f"lib{lib_name}.a"
            if lib_path.exists():
                continue

            dll_path = self._system_dll_path(dll_name)
            if not dll_path:
                self.logger.error(f"Could not find {dll_name} in the Windows system directory")
                success = False
                continue

            exports = self._pe_export_names(dll_path)
            if not exports:
                self.logger.error(f"No exports found in {dll_path}")
                success = False
                continue

            if not self._write_import_lib(zig_path, machine, lib_dir, lib_name, dll_name, exports):
                success = False

        for lib_name, dll_name in WINDOWS_STUB_IMPORT_DLLS.items():
            lib_path = lib_dir / f"lib{lib_name}.a"
            if lib_path.exists():
                continue
            if not self._write_import_lib(zig_path, machine, lib_dir, lib_name, dll_name, []):
                # Empty .def files are rejected by some dlltool builds. An empty
                # archive still satisfies -l<name> when no symbols are used.
                if not self._write_empty_archive(zig_path, lib_path):
                    success = False

        if success:
            self.logger.success(f"Windows import libraries generated at {lib_dir}")
        return success

    def _dlltool_machine(self, rust_target: str) -> str:
        """Return Zig dlltool machine name for a Rust target."""
        if rust_target.startswith("aarch64-"):
            return "arm64"
        if rust_target.startswith("x86_64-"):
            return "i386:x86-64"
        raise ValueError(f"Unsupported Windows target: {rust_target}")

    def _system_dll_path(self, dll_name: str) -> Optional[Path]:
        """Locate a DLL in the native Windows system directory."""
        system_root = Path(os.environ.get("SystemRoot", r"C:\Windows"))
        candidates = [
            system_root / "System32" / dll_name,
            system_root / "Sysnative" / dll_name,
        ]
        for candidate in candidates:
            if candidate.exists():
                return candidate
        return None

    def _write_import_lib(
        self,
        zig_path: Path,
        machine: str,
        lib_dir: Path,
        lib_name: str,
        dll_name: str,
        exports: List[str],
    ) -> bool:
        """Write a .def file and compile it into a GNU import archive."""
        def_path = lib_dir / f"lib{lib_name}.def"
        lib_path = lib_dir / f"lib{lib_name}.a"

        lines = [f"LIBRARY {dll_name}", "EXPORTS"]
        lines.extend(f"    {name}" for name in sorted(set(exports)))
        def_path.write_text("\n".join(lines) + "\n", encoding="utf-8")

        result = subprocess.run(
            [
                str(zig_path),
                "dlltool",
                "-m",
                machine,
                "-d",
                str(def_path),
                "-l",
                str(lib_path),
            ],
            capture_output=True,
            text=True,
        )
        if result.returncode == 0 and lib_path.exists():
            return True

        self.logger.debug(result.stderr.strip())
        if lib_path.exists():
            lib_path.unlink()
        return False

    def _write_empty_archive(self, zig_path: Path, lib_path: Path) -> bool:
        """Create an empty GNU archive."""
        result = subprocess.run(
            [str(zig_path), "ar", "rcs", str(lib_path)],
            capture_output=True,
            text=True,
        )
        if result.returncode == 0 and lib_path.exists():
            return True
        self.logger.error(f"Failed to create {lib_path.name}: {result.stderr}")
        return False

    def _pe_export_names(self, dll_path: Path) -> List[str]:
        """Extract exported symbol names from a PE DLL."""
        data = dll_path.read_bytes()
        if len(data) < 0x40 or data[:2] != b"MZ":
            return []

        pe_offset = struct.unpack_from("<I", data, 0x3C)[0]
        if pe_offset + 24 > len(data) or data[pe_offset:pe_offset + 4] != b"PE\0\0":
            return []

        section_count = struct.unpack_from("<H", data, pe_offset + 6)[0]
        optional_size = struct.unpack_from("<H", data, pe_offset + 20)[0]
        optional_offset = pe_offset + 24
        if optional_offset + optional_size > len(data):
            return []

        magic = struct.unpack_from("<H", data, optional_offset)[0]
        if magic == 0x10B:
            data_directory_offset = optional_offset + 96
        elif magic == 0x20B:
            data_directory_offset = optional_offset + 112
        else:
            return []

        if data_directory_offset + 8 > len(data):
            return []
        export_rva, _export_size = struct.unpack_from("<II", data, data_directory_offset)
        if export_rva == 0:
            return []

        sections = []
        section_offset = optional_offset + optional_size
        for i in range(section_count):
            offset = section_offset + i * 40
            if offset + 40 > len(data):
                return []
            virtual_size, virtual_address, raw_size, raw_pointer = struct.unpack_from(
                "<IIII", data, offset + 8
            )
            sections.append((virtual_address, max(virtual_size, raw_size), raw_pointer))

        export_offset = self._rva_to_offset(export_rva, sections)
        if export_offset is None or export_offset + 40 > len(data):
            return []

        fields = struct.unpack_from("<IIHHIIIIIII", data, export_offset)
        name_count = fields[7]
        names_rva = fields[9]
        names_offset = self._rva_to_offset(names_rva, sections)
        if names_offset is None:
            return []

        exports: List[str] = []
        for i in range(name_count):
            name_rva_offset = names_offset + i * 4
            if name_rva_offset + 4 > len(data):
                break
            name_rva = struct.unpack_from("<I", data, name_rva_offset)[0]
            name_offset = self._rva_to_offset(name_rva, sections)
            if name_offset is None:
                continue
            name = self._read_c_string(data, name_offset)
            if name:
                exports.append(name)
        return exports

    def _rva_to_offset(self, rva: int, sections: List[tuple]) -> Optional[int]:
        """Convert a PE RVA into a file offset."""
        for virtual_address, size, raw_pointer in sections:
            if virtual_address <= rva < virtual_address + size:
                return raw_pointer + (rva - virtual_address)
        return None

    def _read_c_string(self, data: bytes, offset: int) -> str:
        """Read a null-terminated ASCII string from bytes."""
        end = data.find(b"\0", offset)
        if end == -1:
            return ""
        try:
            return data[offset:end].decode("ascii")
        except UnicodeDecodeError:
            return ""

    # ==================== cargo-zigbuild Installation ====================

    def is_cargo_zigbuild_installed(self) -> bool:
        """Check if cargo-zigbuild is installed."""
        # Check if cargo-zigbuild binary exists in cargo bin
        cargo_zigbuild = self.cargo_home / "bin" / self._exe_name("cargo-zigbuild")
        if cargo_zigbuild.exists():
            return True

        # Fallback: check if it's in PATH
        env = self.get_env()
        try:
            result = subprocess.run(
                [self._resolve_command("cargo-zigbuild", env), "--version"],
                capture_output=True,
                text=True,
                env=env,
            )
            return result.returncode == 0
        except FileNotFoundError:
            return False

    def _ensure_default_toolchain(self) -> bool:
        """Ensure rustup has a default toolchain configured."""
        env = self.get_env()
        rustup_path = self.get_rustup_path()
        if not rustup_path:
            return False
        try:
            result = subprocess.run(
                [str(rustup_path), "default"],
                capture_output=True,
                text=True,
                env=env,
            )
            if result.returncode != 0 or "no default" in result.stderr.lower() or "no default" in result.stdout.lower():
                self.logger.info("No default Rust toolchain configured. Setting up stable...")
                setup_result = subprocess.run(
                    [str(rustup_path), "default", "stable"],
                    capture_output=True,
                    text=True,
                    env=env,
                )
                if setup_result.returncode != 0:
                    self.logger.error(f"Failed to set default toolchain: {setup_result.stderr}")
                    return False
                self.logger.success("Default Rust toolchain set to stable")
            return True
        except FileNotFoundError:
            return False

    def install_cargo_zigbuild(self) -> bool:
        """Install cargo-zigbuild."""
        if self.is_cargo_zigbuild_installed():
            self.logger.success("cargo-zigbuild is already installed")
            return True

        if not self.is_rust_installed():
            self.logger.error("Rust must be installed first")
            return False

        if not self._ensure_default_toolchain():
            return False

        self.logger.info("Installing cargo-zigbuild...")

        env = self.get_env()
        cargo_path = self.get_cargo_path()
        if not cargo_path:
            self.logger.error("cargo not found. Please install Rust first.")
            return False

        try:
            result = subprocess.run(
                [str(cargo_path), "install", "cargo-zigbuild"],
                capture_output=True,
                text=True,
                env=env,
            )

            if result.returncode == 0:
                self.logger.success("cargo-zigbuild installed successfully")
                return True
            else:
                self.logger.error(f"Failed to install cargo-zigbuild: {result.stderr}")
                return False

        except FileNotFoundError:
            self.logger.error("cargo not found. Please install Rust first.")
            return False

    # ==================== cargo-xwin Installation ====================

    def is_cargo_xwin_installed(self) -> bool:
        """Check if cargo-xwin is installed."""
        # Check if cargo-xwin binary exists in cargo bin
        cargo_xwin = self.cargo_home / "bin" / self._exe_name("cargo-xwin")
        if cargo_xwin.exists():
            return True

        # Fallback: check if it's in PATH
        env = self.get_env()
        try:
            result = subprocess.run(
                [self._resolve_command("cargo-xwin", env), "--version"],
                capture_output=True,
                text=True,
                env=env,
            )
            return result.returncode == 0
        except FileNotFoundError:
            return False

    def install_cargo_xwin(self) -> bool:
        """Install cargo-xwin for Windows MSVC cross-compilation."""
        if self.is_cargo_xwin_installed():
            self.logger.success("cargo-xwin is already installed")
            return True

        if not self.is_rust_installed():
            self.logger.error("Rust must be installed first")
            return False

        if not self._ensure_default_toolchain():
            return False

        self.logger.info("Installing cargo-xwin...")

        env = self.get_env()
        cargo_path = self.get_cargo_path()
        if not cargo_path:
            self.logger.error("cargo not found. Please install Rust first.")
            return False

        try:
            result = subprocess.run(
                [str(cargo_path), "install", "cargo-xwin"],
                capture_output=True,
                text=True,
                env=env,
            )

            if result.returncode == 0:
                self.logger.success("cargo-xwin installed successfully")
                return True
            else:
                self.logger.error(f"Failed to install cargo-xwin: {result.stderr}")
                return False

        except FileNotFoundError:
            self.logger.error("cargo not found. Please install Rust first.")
            return False

    # ==================== clang/lld Detection ====================

    def is_clang_installed(self) -> bool:
        """Check if clang is installed."""
        try:
            result = subprocess.run(
                ["clang", "--version"],
                capture_output=True,
                text=True,
            )
            return result.returncode == 0
        except FileNotFoundError:
            return False

    def is_lld_installed(self) -> bool:
        """Check if lld (LLVM linker) is installed."""
        # Try lld-link first (used by cargo-xwin on some systems)
        for cmd in ["lld-link", "ld.lld", "lld"]:
            try:
                result = subprocess.run(
                    [cmd, "--version"],
                    capture_output=True,
                    text=True,
                )
                if result.returncode == 0:
                    return True
            except FileNotFoundError:
                continue
        return False

    def is_llvm_lib_installed(self) -> bool:
        """Check if llvm-lib is installed (needed by cargo-xwin for .lib generation)."""
        try:
            result = subprocess.run(
                ["llvm-lib", "--version"],
                capture_output=True,
                text=True,
            )
            return result.returncode == 0
        except FileNotFoundError:
            return False

    def is_clang_cl_installed(self) -> bool:
        """Check if clang-cl is installed (needed for Windows ARM64 cross-compilation)."""
        try:
            result = subprocess.run(
                ["clang-cl", "--version"],
                capture_output=True,
                text=True,
            )
            return result.returncode == 0
        except FileNotFoundError:
            return False

    # ==================== macOS SDK Installation ====================

    def is_macos_sdk_installed(self) -> bool:
        """Check if macOS SDK is installed."""
        return self.sdk_dir.exists() and self.sdk_dir.is_dir()

    def install_macos_sdk(self) -> bool:
        """Install macOS SDK for cross-compilation."""
        if self.is_macos_sdk_installed():
            self.logger.success(f"macOS SDK is already installed at {self.sdk_dir}")
            return True

        # Only needed on Linux
        if self.config.host_os != "linux":
            self.logger.info("macOS SDK not needed on this platform")
            return True

        self.ensure_tools_dir()

        # Download SDK
        archive_name = f"MacOSX{self.config.macos_sdk_version}.sdk.tar.xz"
        archive_path = self.tools_dir / archive_name

        if not archive_path.exists():
            local_cache = Path.home() / ".rustbuilder" / archive_name
            if local_cache.exists():
                self.logger.info(f"Using cached {archive_name} from {local_cache.parent}")
                shutil.copy2(local_cache, archive_path)
            elif not self.download_file(
                self.config.macos_sdk_url, archive_path, "macOS SDK"
            ):
                return False

        # Extract SDK
        if not self.extract_tar_xz(archive_path, self.tools_dir):
            return False

        if self.sdk_dir.exists():
            self.logger.success(f"macOS SDK installed at {self.sdk_dir}")
            return True
        else:
            self.logger.error("macOS SDK installation failed")
            return False

    # ==================== Utility Methods ====================

    def download_file(self, url: str, dest: Path, desc: str = "file") -> bool:
        """Download a file with progress indication."""
        self.logger.info(f"Downloading {desc}...")
        self.logger.info(f"  URL: {url}")

        try:
            ctx = ssl.create_default_context()

            with urllib.request.urlopen(url, context=ctx) as response:
                total_size = int(response.headers.get("content-length", 0))
                downloaded = 0
                chunk_size = 8192

                with open(dest, "wb") as f:
                    while True:
                        chunk = response.read(chunk_size)
                        if not chunk:
                            break
                        f.write(chunk)
                        downloaded += len(chunk)

                        if total_size > 0:
                            percent = (downloaded / total_size) * 100
                            mb_downloaded = downloaded / (1024 * 1024)
                            mb_total = total_size / (1024 * 1024)
                            print(
                                f"\r  Progress: {mb_downloaded:.1f}/{mb_total:.1f} MB ({percent:.1f}%)",
                                end="",
                                flush=True,
                            )

                print()  # New line after progress
                self.logger.success(f"Downloaded {desc}")
                return True

        except Exception as e:
            self.logger.error(f"Failed to download {desc}: {e}")
            if dest.exists():
                dest.unlink()
            return False

    def _is_safe_path_for_deletion(self, path: Path) -> bool:
        """Check if a path is safe to delete (within tools_dir and not a symlink escape)."""
        try:
            # Resolve symlinks to get the real path
            resolved_path = path.resolve()
            tools_dir_resolved = self.tools_dir.resolve()

            # Ensure the resolved path is within tools_dir
            if not (str(resolved_path).startswith(str(tools_dir_resolved) + os.sep) or
                    resolved_path == tools_dir_resolved):
                return False

            # If the path is a symlink, also check that the target is within tools_dir
            if path.is_symlink():
                link_target = path.readlink()
                if link_target.is_absolute():
                    target_resolved = link_target.resolve()
                else:
                    target_resolved = (path.parent / link_target).resolve()

                if not (str(target_resolved).startswith(str(tools_dir_resolved) + os.sep) or
                        target_resolved == tools_dir_resolved):
                    return False

            return True
        except (ValueError, OSError):
            return False

    def _is_safe_tar_member(self, member: tarfile.TarInfo, dest_dir: Path) -> bool:
        """Check if a tar member is safe to extract (no path traversal)."""
        # Reject absolute paths
        if member.name.startswith('/'):
            return False

        # Reject paths with parent directory references
        if '..' in member.name.split('/'):
            return False

        # Resolve the final path and ensure it's within dest_dir
        try:
            dest_dir_resolved = dest_dir.resolve()
            member_path = (dest_dir / member.name).resolve()
            # Check if the resolved path is within the destination directory
            return str(member_path).startswith(str(dest_dir_resolved) + os.sep) or member_path == dest_dir_resolved
        except (ValueError, OSError):
            return False

    def extract_tar_xz(self, archive: Path, dest_dir: Path) -> bool:
        """Extract a .tar.xz archive with path traversal protection."""
        self.logger.info(f"Extracting {archive.name}...")
        try:
            with tarfile.open(archive, "r:xz") as tar:
                # Validate all members before extraction
                for member in tar.getmembers():
                    if not self._is_safe_tar_member(member, dest_dir):
                        self.logger.error(f"Unsafe path in archive: {member.name}")
                        return False
                    # Also reject symbolic links pointing outside
                    if member.issym() or member.islnk():
                        link_target = member.linkname
                        if link_target.startswith('/') or '..' in link_target.split('/'):
                            self.logger.error(f"Unsafe symlink in archive: {member.name} -> {link_target}")
                            return False

                # Safe to extract
                tar.extractall(path=dest_dir)
            self.logger.success("Extraction complete")
            return True
        except Exception as e:
            self.logger.error(f"Failed to extract archive: {e}")
            return False

    def _is_safe_zip_member(self, name: str, dest_dir: Path) -> bool:
        """Check if a zip member is safe to extract (no path traversal)."""
        if name.startswith(("/", "\\")):
            return False
        parts = Path(name).parts
        if ".." in parts:
            return False
        try:
            dest_dir_resolved = dest_dir.resolve()
            member_path = (dest_dir / name).resolve()
            return str(member_path).startswith(str(dest_dir_resolved) + os.sep) or member_path == dest_dir_resolved
        except (ValueError, OSError):
            return False

    def extract_zip(self, archive: Path, dest_dir: Path) -> bool:
        """Extract a .zip archive with path traversal protection."""
        self.logger.info(f"Extracting {archive.name}...")
        try:
            with zipfile.ZipFile(archive) as zf:
                for info in zf.infolist():
                    if not self._is_safe_zip_member(info.filename, dest_dir):
                        self.logger.error(f"Unsafe path in archive: {info.filename}")
                        return False
                zf.extractall(dest_dir)
            self.logger.success("Extraction complete")
            return True
        except Exception as e:
            self.logger.error(f"Failed to extract archive: {e}")
            return False

    # ==================== Setup Methods ====================

    def setup_rust(self) -> bool:
        """Install Rust toolchain."""
        self.logger.header("Setting up Rust toolchain")
        return self.install_rust()

    def setup_cross_compile(self) -> bool:
        """Install all required tools for cross-compilation (zigbuild + macOS SDK)."""
        self.logger.header("Setting up cross-compilation tools")

        success = True

        if not self.install_zig():
            success = False

        if not self.install_cargo_zigbuild():
            success = False

        if not self.install_macos_sdk():
            success = False

        if success:
            self.logger.success("All cross-compilation tools installed!")
        else:
            self.logger.error("Some tools failed to install")

        return success

    def setup_windows_cross(self) -> bool:
        """Install tools for Windows builds."""
        self.logger.header("Setting up Windows cross-compilation tools")

        success = True

        if self.config.host_os == "windows":
            if not self.install_zig():
                success = False

            for rust_target in self._windows_gnullvm_triples():
                if not self.install_rust_toolchain(f"stable-{rust_target}"):
                    success = False
                if not self.install_windows_import_libs(rust_target):
                    success = False

            if success:
                self.logger.success("Windows GNU/LLVM build tools are ready!")
            else:
                self.logger.error("Some Windows GNU/LLVM build tools failed to install")
            return success

        if not self.install_cargo_xwin():
            success = False

        if not self.is_clang_installed():
            self.logger.warning("clang is not installed. Required for cargo-xwin.")
            self.logger.info("  Install with: apt install clang  (or your package manager)")
            success = False

        if not self.is_lld_installed():
            self.logger.warning("lld is not installed. Required for cargo-xwin.")
            self.logger.info("  Install with: apt install lld  (or your package manager)")
            success = False

        if not self.is_llvm_lib_installed():
            self.logger.warning("llvm-lib is not installed. Required for cargo-xwin.")
            self.logger.info("  Install with: apt install llvm  (or your package manager)")
            self.logger.info("  If llvm-lib-XX exists but llvm-lib doesn't, create a symlink:")
            self.logger.info("    sudo ln -s llvm-lib-18 /usr/bin/llvm-lib")
            success = False

        if not self.is_clang_cl_installed():
            self.logger.warning("clang-cl is not installed. Required for Windows ARM64 builds.")
            self.logger.info("  Install with: apt install clang-tools-18  (or matching version)")
            self.logger.info("  If clang-cl-XX exists but clang-cl doesn't, create a symlink:")
            self.logger.info("    sudo ln -s clang-cl-18 /usr/bin/clang-cl")
            success = False

        if success:
            self.logger.success("All Windows cross-compilation tools ready!")
        else:
            self.logger.error("Some Windows cross-compilation tools are missing")
            self.logger.info("  Or run: sudo ./install_windows_build_deps.sh")

        return success

    def setup_all(self) -> bool:
        """Install all required tools (Rust + cross-compilation). Use --setup-windows for Windows tools."""
        success = True

        # Install Rust first
        if not self.setup_rust():
            success = False
            return success  # Can't continue without Rust

        # Install cross-compilation tools (zigbuild + macOS SDK)
        if not self.setup_cross_compile():
            success = False

        return success

    def _append_env_flags(self, env: dict, key: str, flags: List[str]) -> None:
        """Append compiler flags to an environment variable."""
        existing = env.get(key, "").strip()
        addition = " ".join(flags)
        if not existing:
            env[key] = addition
        elif addition not in existing:
            env[key] = f"{existing} {addition}"

    def get_env(self) -> dict:
        """Get environment variables for build process."""
        env = os.environ.copy()

        # Set Rust environment
        env["CARGO_HOME"] = str(self.cargo_home)
        env["RUSTUP_HOME"] = str(self.rustup_home)

        # Build PATH with all tools
        path_parts = []

        # Add cargo bin
        cargo_bin = self.cargo_home / "bin"
        if cargo_bin.exists():
            path_parts.append(str(cargo_bin))

        # Add zig
        zig_path = self.get_zig_path()
        if zig_path:
            path_parts.append(str(zig_path.parent))

        # Add original PATH
        path_key = self._path_env_key(env)
        path_parts.append(env.get(path_key, ""))

        env[path_key] = os.pathsep.join(path_parts)

        # Set SDKROOT for macOS cross-compilation
        if self.sdk_dir.exists():
            env["SDKROOT"] = str(self.sdk_dir)

        if self.config.host_os == "windows":
            ar_wrapper = self.windows_ar_wrapper_path()
            for rust_target in self._windows_gnullvm_triples():
                cc_wrapper = self.windows_cc_wrapper_path(rust_target)
                normalized = rust_target.replace("-", "_")
                cargo_key = rust_target.upper().replace("-", "_")

                if cc_wrapper and cc_wrapper.exists():
                    env[f"CC_{normalized}"] = str(cc_wrapper)
                if ar_wrapper.exists():
                    env[f"AR_{normalized}"] = str(ar_wrapper)

                env[f"CARGO_TARGET_{cargo_key}_LINKER"] = "rust-lld"

                lib_dir = self.windows_import_lib_dir(rust_target)
                if lib_dir.exists():
                    self._append_env_flags(
                        env,
                        f"CARGO_TARGET_{cargo_key}_RUSTFLAGS",
                        [
                            "-C",
                            "target-feature=+crt-static",
                            "-L",
                            f"native={lib_dir.resolve()}",
                        ],
                    )

        return env

    def print_status(self) -> None:
        """Print status of all tools."""
        self.logger.header("Tool Status")

        # Rust
        if self.is_rust_installed():
            cargo_path = self.get_cargo_path()
            self.logger.success(f"Rust: {cargo_path}")
        else:
            self.logger.warning("Rust: Not installed")

        # Zig
        if self.is_zig_installed():
            zig_path = self.get_zig_path()
            self.logger.success(f"Zig: {zig_path}")
        else:
            self.logger.warning("Zig: Not installed")

        # cargo-zigbuild
        if self.is_cargo_zigbuild_installed():
            self.logger.success("cargo-zigbuild: Installed")
        else:
            self.logger.warning("cargo-zigbuild: Not installed")

        # macOS SDK
        if self.is_macos_sdk_installed():
            self.logger.success(f"macOS SDK: {self.sdk_dir}")
        else:
            if self.config.host_os == "linux":
                self.logger.warning("macOS SDK: Not installed")
            else:
                self.logger.info("macOS SDK: Not needed on this platform")

        if self.config.host_os == "windows":
            self.logger.info("cargo-xwin/clang-cl: Not needed for Windows GNU/LLVM builds")
            for rust_target in self._windows_gnullvm_triples():
                toolchain = f"stable-{rust_target}"
                if self.is_rust_toolchain_installed(toolchain):
                    self.logger.success(f"{toolchain}: Installed")
                else:
                    self.logger.warning(f"{toolchain}: Not installed")

                if self.is_windows_import_libs_installed(rust_target):
                    self.logger.success(f"winlibs {rust_target}: Installed")
                else:
                    self.logger.warning(f"winlibs {rust_target}: Not installed")
        else:
            # cargo-xwin (Windows cross-compilation)
            if self.is_cargo_xwin_installed():
                self.logger.success("cargo-xwin: Installed")
            else:
                self.logger.warning("cargo-xwin: Not installed")

            # clang
            if self.is_clang_installed():
                self.logger.success("clang: Installed")
            else:
                self.logger.warning("clang: Not installed")

            # lld
            if self.is_lld_installed():
                self.logger.success("lld: Installed")
            else:
                self.logger.warning("lld: Not installed")

            # llvm-lib
            if self.is_llvm_lib_installed():
                self.logger.success("llvm-lib: Installed")
            else:
                self.logger.warning("llvm-lib: Not installed")

            # clang-cl
            if self.is_clang_cl_installed():
                self.logger.success("clang-cl: Installed")
            else:
                self.logger.warning("clang-cl: Not installed")
