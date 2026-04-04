from __future__ import annotations

import argparse
import shutil
import stat
from dataclasses import dataclass
from pathlib import Path


@dataclass(frozen=True)
class Config:
    target_os: str
    arch: str
    artifact_name: str
    staging_root: Path
    app_binary: Path
    zju_connect: Path
    wintun: Path | None
    windows_runtime_root: Path | None
    app_name: str


class ParsedArgs(argparse.Namespace):
    target_os: str = ""
    arch: str = ""
    artifact_name: str = ""
    staging_root: str = ""
    app_binary: str = ""
    zju_connect: str = ""
    wintun: str | None = None
    windows_runtime_root: str | None = None
    app_name: str = "zju-connect-gui"


def parse_args() -> Config:
    parser = argparse.ArgumentParser(
        description="Stage packaged zju-connect-gui artifacts with required runtime layout."
    )
    _ = parser.add_argument(
        "--target-os",
        required=True,
        choices=["windows", "linux", "macos"],
        help="Packaging target operating system",
    )
    _ = parser.add_argument(
        "--arch", required=True, help="Packaging target architecture"
    )
    _ = parser.add_argument(
        "--artifact-name", required=True, help="Stable artifact directory name"
    )
    _ = parser.add_argument(
        "--staging-root", required=True, help="Root directory for staged packages"
    )
    _ = parser.add_argument(
        "--app-binary", required=True, help="Built desktop application binary path"
    )
    _ = parser.add_argument(
        "--zju-connect", required=True, help="Built upstream zju-connect binary path"
    )
    _ = parser.add_argument("--wintun", help="Path to wintun.dll for Windows packages")
    _ = parser.add_argument(
        "--windows-runtime-root",
        help="Path to the extracted Windows GTK runtime root for Windows packages",
    )
    _ = parser.add_argument(
        "--app-name", default="zju-connect-gui", help="Desktop app base name"
    )
    namespace = parser.parse_args(namespace=ParsedArgs())
    wintun = Path(namespace.wintun).resolve() if namespace.wintun else None
    windows_runtime_root = (
        Path(namespace.windows_runtime_root).resolve()
        if namespace.windows_runtime_root
        else None
    )
    return Config(
        target_os=namespace.target_os,
        arch=namespace.arch,
        artifact_name=namespace.artifact_name,
        staging_root=Path(namespace.staging_root).resolve(),
        app_binary=Path(namespace.app_binary).resolve(),
        zju_connect=Path(namespace.zju_connect).resolve(),
        wintun=wintun,
        windows_runtime_root=windows_runtime_root,
        app_name=namespace.app_name,
    )


def ensure_exists(path: Path, description: str) -> None:
    if not path.exists():
        raise FileNotFoundError(f"Missing {description}: {path}")


def ensure_executable(path: Path) -> None:
    current_mode = path.stat().st_mode
    path.chmod(current_mode | stat.S_IXUSR | stat.S_IXGRP | stat.S_IXOTH)


def copy_file(source: Path, destination: Path) -> None:
    destination.parent.mkdir(parents=True, exist_ok=True)
    _ = shutil.copy2(source, destination)


def copy_tree(source: Path, destination: Path) -> None:
    if not source.exists():
        return
    destination.parent.mkdir(parents=True, exist_ok=True)
    if destination.exists():
        shutil.rmtree(destination)
    _ = shutil.copytree(source, destination)


def verify_packaged_layout(
    stage_dir: Path,
    target_os: str,
    app_name: str,
    windows_runtime_root: Path | None,
) -> None:
    if target_os == "windows":
        required = [
            stage_dir / f"{app_name}.exe",
            stage_dir / "bin" / "zju-connect.exe",
            stage_dir / "bin" / "wintun.dll",
        ]
        if windows_runtime_root is not None:
            required.extend(
                [
                    stage_dir / "libgtk-4-1.dll",
                    stage_dir / "share" / "gtk-4.0",
                    stage_dir / "share" / "glib-2.0" / "schemas",
                ]
            )
    else:
        required = [
            stage_dir / app_name,
            stage_dir / "bin" / "zju-connect",
        ]

    for path in required:
        ensure_exists(path, "packaged runtime path")


def main() -> None:
    args = parse_args()

    app_binary = args.app_binary
    zju_connect_source = args.zju_connect
    ensure_exists(app_binary, "desktop application binary")
    ensure_exists(zju_connect_source, "built zju-connect helper")

    stage_dir = args.staging_root / args.artifact_name
    if stage_dir.exists():
        shutil.rmtree(stage_dir)
    stage_dir.mkdir(parents=True, exist_ok=True)

    if args.target_os == "windows":
        staged_app = stage_dir / f"{args.app_name}.exe"
        helper_name = "zju-connect.exe"
    else:
        staged_app = stage_dir / args.app_name
        helper_name = "zju-connect"

    copy_file(app_binary, staged_app)
    runtime_root = stage_dir
    helper_dir = stage_dir / "bin"

    helper_dir.mkdir(parents=True, exist_ok=True)
    helper_destination = helper_dir / helper_name
    copy_file(zju_connect_source, helper_destination)

    if args.target_os != "windows":
        ensure_executable(helper_destination)
        if staged_app.is_file():
            ensure_executable(staged_app)
        else:
            ensure_executable(runtime_root / args.app_name)

    if args.target_os == "windows":
        if not args.wintun:
            raise ValueError("Windows packaging requires --wintun")
        wintun_source = args.wintun
        ensure_exists(wintun_source, "wintun.dll")
        copy_file(wintun_source, helper_dir / "wintun.dll")

        if args.windows_runtime_root is not None:
            runtime_source = args.windows_runtime_root
            ensure_exists(runtime_source, "Windows GTK runtime root")

            runtime_bin_dir = runtime_source / "bin"
            ensure_exists(runtime_bin_dir, "Windows GTK runtime bin directory")
            for dll_path in sorted(runtime_bin_dir.glob("*.dll")):
                copy_file(dll_path, stage_dir / dll_path.name)

            for relative_dir in [
                Path("etc"),
                Path("share"),
                Path("lib") / "gdk-pixbuf-2.0",
                Path("lib") / "girepository-1.0",
                Path("lib") / "gtk-4.0",
            ]:
                copy_tree(runtime_source / relative_dir, stage_dir / relative_dir)

    verify_packaged_layout(
        stage_dir, args.target_os, args.app_name, args.windows_runtime_root
    )

    print(f"staged {args.target_os}/{args.arch} package at {stage_dir}")


if __name__ == "__main__":
    main()
