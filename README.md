# SmartFile Renamer (Tauri)

Aplikasi desktop batch file renamer berbasis Tauri (frontend HTML + TypeScript, backend Rust), dengan preview, validasi konflik, dry run, dan undo rename terakhir.

## Fitur

- Layout 2 kolom: file list + panel rename.
- Drag & drop file/folder.
- Preview `Before/After` dengan highlight konflik.
- Validasi konflik sebelum apply.
- Rename aman (2-phase temporary rename + rollback best-effort).
- Dry run tanpa mengubah file.
- Undo last rename dari log mapping terakhir.

## Prasyarat

- Node.js 18+ dan npm
- Rust (rustup, cargo)
- Tauri prerequisites sesuai OS:
  - macOS: Xcode Command Line Tools
  - Windows: Visual Studio Build Tools (Desktop development with C++) + WebView2

## Menjalankan (Development)

```bash
npm install
npm run tauri dev
```

## Build Installer

```bash
npm run tauri build
```

Output build ada di folder:

- `src-tauri/target/release/bundle/`

Catatan lintas platform:

- Build paket Windows (`.msi/.exe`) sebaiknya dijalankan di Windows.
- Build paket macOS (`.app/.dmg`) sebaiknya dijalankan di macOS.

## Command Backend

- `build_rename_plan(request)`
- `validate_rename_plan({ operations })`
- `apply_rename_plan({ operations, dry_run })`
- `undo_last_rename()`

## Struktur Project

- `src/` frontend TypeScript
- `src-tauri/src/main.rs` backend Rust commands
- `src-tauri/tauri.conf.json` konfigurasi Tauri

## Push ke GitHub

1. Inisialisasi git (jika belum):

```bash
git init
git add .
git commit -m "Initial commit"
```

2. Buat repo kosong di GitHub, lalu hubungkan remote:

```bash
git remote add origin https://github.com/<username>/<repo>.git
git branch -M main
git push -u origin main
```

## GitHub Release Otomatis (Windows .exe)

Project ini sudah punya workflow:

- `.github/workflows/windows-release.yml`

Cara pakai:

1. Pastikan commit terbaru sudah di-push ke `main`.
2. Buat tag versi, lalu push tag:

```bash
git tag v0.1.0
git push origin v0.1.0
```

3. GitHub Actions akan build installer Windows (`.exe` / NSIS) dan upload ke halaman **Releases** untuk tag tersebut.

## Rilis untuk User Windows

Opsi paling sederhana:

1. Clone repo di mesin Windows.
2. Jalankan `npm install`.
3. Jalankan `npm run tauri build`.
4. Bagikan file installer dari `src-tauri/target/release/bundle/`.
