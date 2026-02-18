#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};
use tauri::State;

struct AppState {
    last_rename: Mutex<Option<Vec<RenameMapping>>>,
}

#[derive(Debug, Deserialize)]
struct BuildRenamePlanRequest {
    file_paths: Vec<String>,
    prefix: String,
    start_number: u64,
    padding: Option<usize>,
    suffix: Option<String>,
    keep_extension: Option<bool>,
    sort_by: Option<SortBy>,
}

#[derive(Debug, Deserialize)]
struct ValidateRenamePlanRequest {
    operations: Vec<RenameOperation>,
}

#[derive(Debug, Deserialize)]
struct ApplyRenamePlanRequest {
    operations: Vec<RenameOperation>,
    dry_run: Option<bool>,
}

#[derive(Debug, Deserialize, Clone, Copy)]
#[serde(rename_all = "snake_case")]
enum SortBy {
    Name,
    ModifiedTime,
    Size,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RenameOperation {
    source_path: String,
    target_path: String,
    source_name: String,
    target_name: String,
    extension: String,
    size_bytes: u64,
    modified_unix_ms: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RenameMapping {
    from_path: String,
    to_path: String,
}

#[derive(Debug, Serialize)]
struct RenamePlan {
    operations: Vec<RenameOperation>,
}

#[derive(Debug, Clone, Serialize)]
struct ConflictDetail {
    source_path: String,
    target_path: String,
    message: String,
    kind: String,
}

#[derive(Debug, Serialize)]
struct ValidateRenameResult {
    total_files: usize,
    safe_files: usize,
    conflict_files: usize,
    conflicts: Vec<ConflictDetail>,
}

#[derive(Debug, Serialize)]
struct ApplyRenameResult {
    dry_run: bool,
    total_files: usize,
    renamed_files: usize,
    skipped_files: usize,
    conflict_files: usize,
    conflicts: Vec<ConflictDetail>,
    rolled_back: bool,
    sample_targets: Vec<String>,
    message: String,
}

#[derive(Debug, Serialize)]
struct UndoRenameResult {
    restored_files: usize,
    restored_paths: Vec<String>,
    message: String,
}

#[derive(Debug, Clone)]
struct SourceRecord {
    path: PathBuf,
    source_name: String,
    extension: String,
    size_bytes: u64,
    modified_unix_ms: i64,
}

#[derive(Debug)]
struct RenamePair {
    source: PathBuf,
    target: PathBuf,
}

#[derive(Debug)]
struct StagedMove {
    original: PathBuf,
    temp: PathBuf,
    target: PathBuf,
    committed: bool,
}

#[derive(Debug)]
struct TransactionError {
    message: String,
}

#[tauri::command]
fn build_rename_plan(request: BuildRenamePlanRequest) -> Result<RenamePlan, String> {
    if request.file_paths.is_empty() {
        return Err("Tidak ada file yang dipilih.".to_string());
    }

    if request.start_number == 0 {
        return Err("Start number minimal 1.".to_string());
    }

    let prefix = sanitize_token(&request.prefix, "Prefix")?;
    let suffix = sanitize_token(request.suffix.as_deref().unwrap_or(""), "Suffix")?;
    let keep_extension = request.keep_extension.unwrap_or(true);
    let padding = request.padding.unwrap_or(0);
    let sort_by = request.sort_by.unwrap_or(SortBy::Name);

    let mut source_records = collect_source_records(&request.file_paths)?;
    sort_source_records(&mut source_records, sort_by);

    let mut operations = Vec::with_capacity(source_records.len());

    for (index, record) in source_records.iter().enumerate() {
        let step = u64::try_from(index).map_err(|_| "Jumlah file terlalu besar.".to_string())?;
        let sequence = request
            .start_number
            .checked_add(step)
            .ok_or_else(|| "Nomor urut melebihi batas u64.".to_string())?;

        let number = if padding > 0 {
            format!("{sequence:0width$}", width = padding)
        } else {
            sequence.to_string()
        };

        let extension_with_dot = if keep_extension && !record.extension.is_empty() {
            format!(".{}", record.extension)
        } else {
            String::new()
        };

        let target_name = format!("{}{}{}{}", prefix, number, suffix, extension_with_dot);
        let parent = record
            .path
            .parent()
            .ok_or_else(|| format!("Path tidak valid: {}", record.path.display()))?;
        let target_path = parent.join(&target_name);

        operations.push(RenameOperation {
            source_path: record.path.to_string_lossy().to_string(),
            target_path: target_path.to_string_lossy().to_string(),
            source_name: record.source_name.clone(),
            target_name,
            extension: record.extension.clone(),
            size_bytes: record.size_bytes,
            modified_unix_ms: record.modified_unix_ms,
        });
    }

    Ok(RenamePlan { operations })
}

#[tauri::command]
fn validate_rename_plan(request: ValidateRenamePlanRequest) -> Result<ValidateRenameResult, String> {
    if request.operations.is_empty() {
        return Ok(ValidateRenameResult {
            total_files: 0,
            safe_files: 0,
            conflict_files: 0,
            conflicts: Vec::new(),
        });
    }

    Ok(validate_operations(&request.operations))
}

#[tauri::command]
fn apply_rename_plan(
    request: ApplyRenamePlanRequest,
    state: State<'_, AppState>,
) -> Result<ApplyRenameResult, String> {
    if request.operations.is_empty() {
        return Err("Rename plan kosong.".to_string());
    }

    let dry_run = request.dry_run.unwrap_or(false);
    let validation = validate_operations(&request.operations);
    let sample_targets = request
        .operations
        .iter()
        .take(3)
        .map(|operation| operation.target_name.clone())
        .collect::<Vec<String>>();

    if dry_run {
        return Ok(ApplyRenameResult {
            dry_run: true,
            total_files: validation.total_files,
            renamed_files: 0,
            skipped_files: validation.total_files,
            conflict_files: validation.conflict_files,
            conflicts: validation.conflicts,
            rolled_back: false,
            sample_targets,
            message: "Dry run selesai. Tidak ada file yang diubah.".to_string(),
        });
    }

    if validation.conflict_files > 0 {
        return Ok(ApplyRenameResult {
            dry_run: false,
            total_files: validation.total_files,
            renamed_files: 0,
            skipped_files: validation.total_files,
            conflict_files: validation.conflict_files,
            conflicts: validation.conflicts,
            rolled_back: false,
            sample_targets,
            message: "Rename diblokir karena konflik.".to_string(),
        });
    }

    let pairs = request
        .operations
        .iter()
        .filter(|operation| operation.source_path != operation.target_path)
        .map(|operation| RenamePair {
            source: PathBuf::from(&operation.source_path),
            target: PathBuf::from(&operation.target_path),
        })
        .collect::<Vec<RenamePair>>();

    if pairs.is_empty() {
        return Ok(ApplyRenameResult {
            dry_run: false,
            total_files: request.operations.len(),
            renamed_files: 0,
            skipped_files: request.operations.len(),
            conflict_files: 0,
            conflicts: Vec::new(),
            rolled_back: false,
            sample_targets,
            message: "Tidak ada perubahan nama yang perlu dieksekusi.".to_string(),
        });
    }

    execute_transaction(&pairs).map_err(|error| error.message.clone())?;

    let mappings = pairs
        .iter()
        .map(|pair| RenameMapping {
            from_path: pair.source.to_string_lossy().to_string(),
            to_path: pair.target.to_string_lossy().to_string(),
        })
        .collect::<Vec<RenameMapping>>();

    let mut lock = state
        .last_rename
        .lock()
        .map_err(|_| "Gagal mengakses state log rename.".to_string())?;
    *lock = Some(mappings);

    Ok(ApplyRenameResult {
        dry_run: false,
        total_files: request.operations.len(),
        renamed_files: pairs.len(),
        skipped_files: request.operations.len().saturating_sub(pairs.len()),
        conflict_files: 0,
        conflicts: Vec::new(),
        rolled_back: false,
        sample_targets,
        message: "Rename berhasil.".to_string(),
    })
}

#[tauri::command]
fn undo_last_rename(state: State<'_, AppState>) -> Result<UndoRenameResult, String> {
    let mappings = {
        let lock = state
            .last_rename
            .lock()
            .map_err(|_| "Gagal membaca log rename.".to_string())?;
        lock.clone()
    };

    let mappings = mappings.ok_or_else(|| "Belum ada log rename untuk di-undo.".to_string())?;

    let pairs = mappings
        .iter()
        .map(|mapping| RenamePair {
            source: PathBuf::from(&mapping.to_path),
            target: PathBuf::from(&mapping.from_path),
        })
        .collect::<Vec<RenamePair>>();

    let undo_operations = pairs
        .iter()
        .map(|pair| rename_operation_from_paths(&pair.source, &pair.target))
        .collect::<Result<Vec<RenameOperation>, String>>()?;

    let validation = validate_operations(&undo_operations);
    if validation.conflict_files > 0 {
        let details = validation
            .conflicts
            .into_iter()
            .map(|item| format!("- {}", item.message))
            .collect::<Vec<String>>()
            .join("\n");

        return Err(format!("Undo diblokir karena konflik:\n{details}"));
    }

    execute_transaction(&pairs).map_err(|error| error.message)?;

    let mut lock = state
        .last_rename
        .lock()
        .map_err(|_| "Gagal memperbarui log rename.".to_string())?;
    *lock = None;

    Ok(UndoRenameResult {
        restored_files: pairs.len(),
        restored_paths: pairs
            .iter()
            .map(|pair| pair.target.to_string_lossy().to_string())
            .collect(),
        message: "Undo berhasil.".to_string(),
    })
}

fn sanitize_token(token: &str, label: &str) -> Result<String, String> {
    if token.contains('/') || token.contains('\\') || token.contains('\0') {
        return Err(format!(
            "{} tidak boleh mengandung karakter path separator.",
            label
        ));
    }
    Ok(token.to_string())
}

fn collect_source_records(input_paths: &[String]) -> Result<Vec<SourceRecord>, String> {
    let mut collected = Vec::new();
    let mut seen_paths = HashSet::new();

    for input in input_paths {
        let path = PathBuf::from(input);
        if !path.exists() {
            return Err(format!("Path tidak ditemukan: {}", path.display()));
        }

        if path.is_file() {
            push_source_record(&path, &mut seen_paths, &mut collected)?;
            continue;
        }

        if path.is_dir() {
            let entries = fs::read_dir(&path)
                .map_err(|error| format!("Gagal membaca folder {}: {}", path.display(), error))?;

            for entry_result in entries {
                let entry = entry_result.map_err(|error| {
                    format!("Gagal membaca isi folder {}: {}", path.display(), error)
                })?;
                let child = entry.path();
                if child.is_file() {
                    push_source_record(&child, &mut seen_paths, &mut collected)?;
                }
            }
            continue;
        }

        return Err(format!("Path tidak didukung: {}", path.display()));
    }

    if collected.is_empty() {
        return Err("Tidak ada file valid untuk diproses.".to_string());
    }

    Ok(collected)
}

fn push_source_record(
    path: &Path,
    seen_paths: &mut HashSet<PathBuf>,
    collected: &mut Vec<SourceRecord>,
) -> Result<(), String> {
    let normalized = path.to_path_buf();
    if !seen_paths.insert(normalized.clone()) {
        return Ok(());
    }

    let metadata = fs::metadata(path)
        .map_err(|error| format!("Gagal membaca metadata {}: {}", path.display(), error))?;

    let source_name = path
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .ok_or_else(|| format!("Nama file tidak valid: {}", path.display()))?;

    let extension = path
        .extension()
        .map(|ext| ext.to_string_lossy().to_string())
        .unwrap_or_default();

    let modified_unix_ms = metadata
        .modified()
        .ok()
        .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
        .map(|duration| i64::try_from(duration.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0);

    collected.push(SourceRecord {
        path: normalized,
        source_name,
        extension,
        size_bytes: metadata.len(),
        modified_unix_ms,
    });

    Ok(())
}

fn sort_source_records(records: &mut [SourceRecord], sort_by: SortBy) {
    match sort_by {
        SortBy::Name => {
            records.sort_by(|left, right| {
                left.source_name
                    .to_lowercase()
                    .cmp(&right.source_name.to_lowercase())
                    .then_with(|| left.path.to_string_lossy().cmp(&right.path.to_string_lossy()))
            });
        }
        SortBy::ModifiedTime => {
            records.sort_by(|left, right| {
                left.modified_unix_ms
                    .cmp(&right.modified_unix_ms)
                    .then_with(|| left.source_name.to_lowercase().cmp(&right.source_name.to_lowercase()))
                    .then_with(|| left.path.to_string_lossy().cmp(&right.path.to_string_lossy()))
            });
        }
        SortBy::Size => {
            records.sort_by(|left, right| {
                left.size_bytes
                    .cmp(&right.size_bytes)
                    .then_with(|| left.source_name.to_lowercase().cmp(&right.source_name.to_lowercase()))
                    .then_with(|| left.path.to_string_lossy().cmp(&right.path.to_string_lossy()))
            });
        }
    }
}

fn validate_operations(operations: &[RenameOperation]) -> ValidateRenameResult {
    let total_files = operations.len();
    let mut conflicts: Vec<ConflictDetail> = Vec::new();
    let mut conflict_keys: HashSet<String> = HashSet::new();
    let mut conflict_sources: HashSet<String> = HashSet::new();

    let source_set = operations
        .iter()
        .map(|operation| PathBuf::from(&operation.source_path))
        .collect::<HashSet<PathBuf>>();
    let mut source_map: HashMap<PathBuf, Vec<&RenameOperation>> = HashMap::new();

    let mut targets_map: HashMap<PathBuf, Vec<&RenameOperation>> = HashMap::new();

    for operation in operations {
        let source = PathBuf::from(&operation.source_path);
        let target = PathBuf::from(&operation.target_path);
        source_map.entry(source.clone()).or_default().push(operation);

        if !source.exists() {
            push_conflict(
                &mut conflicts,
                &mut conflict_keys,
                &mut conflict_sources,
                operation,
                "source_missing",
                format!("Sumber tidak ditemukan: {}", source.display()),
            );
            continue;
        }

        if !source.is_file() {
            push_conflict(
                &mut conflicts,
                &mut conflict_keys,
                &mut conflict_sources,
                operation,
                "source_not_file",
                format!("Sumber bukan file: {}", source.display()),
            );
            continue;
        }

        if operation.target_name.trim().is_empty() {
            push_conflict(
                &mut conflicts,
                &mut conflict_keys,
                &mut conflict_sources,
                operation,
                "invalid_target_name",
                "Nama target kosong.".to_string(),
            );
            continue;
        }

        if source == target {
            continue;
        }

        if let Some(parent) = target.parent() {
            if !parent.exists() {
                push_conflict(
                    &mut conflicts,
                    &mut conflict_keys,
                    &mut conflict_sources,
                    operation,
                    "target_parent_missing",
                    format!("Folder target tidak ditemukan: {}", parent.display()),
                );
            }
        }

        targets_map.entry(target.clone()).or_default().push(operation);

        if target.exists() && !source_set.contains(&target) {
            push_conflict(
                &mut conflicts,
                &mut conflict_keys,
                &mut conflict_sources,
                operation,
                "target_exists",
                format!("Target sudah ada di disk: {}", target.display()),
            );
        }
    }

    for (source_path, operation_list) in source_map {
        if operation_list.len() > 1 {
            for operation in operation_list {
                push_conflict(
                    &mut conflicts,
                    &mut conflict_keys,
                    &mut conflict_sources,
                    operation,
                    "duplicate_source",
                    format!(
                        "Sumber dipilih lebih dari sekali: {}",
                        source_path.display()
                    ),
                );
            }
        }
    }

    for (target_path, operation_list) in targets_map {
        if operation_list.len() > 1 {
            for operation in operation_list {
                push_conflict(
                    &mut conflicts,
                    &mut conflict_keys,
                    &mut conflict_sources,
                    operation,
                    "duplicate_target",
                    format!(
                        "Collision internal: lebih dari satu file menuju {}",
                        target_path.display()
                    ),
                );
            }
        }
    }

    let conflict_files = conflict_sources.len();
    let safe_files = total_files.saturating_sub(conflict_files);

    ValidateRenameResult {
        total_files,
        safe_files,
        conflict_files,
        conflicts,
    }
}

fn push_conflict(
    conflicts: &mut Vec<ConflictDetail>,
    keys: &mut HashSet<String>,
    conflict_sources: &mut HashSet<String>,
    operation: &RenameOperation,
    kind: &str,
    message: String,
) {
    let key = format!("{}|{}|{}", operation.source_path, operation.target_path, kind);
    if !keys.insert(key) {
        return;
    }

    conflict_sources.insert(operation.source_path.clone());
    conflicts.push(ConflictDetail {
        source_path: operation.source_path.clone(),
        target_path: operation.target_path.clone(),
        message,
        kind: kind.to_string(),
    });
}

fn execute_transaction(pairs: &[RenamePair]) -> Result<(), TransactionError> {
    let mut staged_moves = Vec::new();

    for (index, pair) in pairs.iter().enumerate() {
        let temp = match generate_temp_path(&pair.source, index) {
            Ok(value) => value,
            Err(error) => {
                let rollback_errors = rollback_all(&staged_moves);
                let rollback_message = if rollback_errors.is_empty() {
                    String::new()
                } else {
                    format!("\nRollback warning:\n- {}", rollback_errors.join("\n- "))
                };
                return Err(TransactionError {
                    message: format!(
                        "Gagal menyiapkan file sementara untuk {}: {}{}",
                        pair.source.display(),
                        error,
                        rollback_message
                    ),
                });
            }
        };

        // Tahap 1: semua sumber dipindah ke nama sementara untuk mencegah overwrite in-place.
        if let Err(error) = fs::rename(&pair.source, &temp) {
            let rollback_errors = rollback_all(&staged_moves);
            let rollback_message = if rollback_errors.is_empty() {
                String::new()
            } else {
                format!("\nRollback warning:\n- {}", rollback_errors.join("\n- "))
            };

            return Err(TransactionError {
                message: format!(
                    "Gagal menyiapkan rename {} -> {}: {}{}",
                    pair.source.display(),
                    pair.target.display(),
                    error,
                    rollback_message
                ),
            });
        }

        staged_moves.push(StagedMove {
            original: pair.source.clone(),
            temp,
            target: pair.target.clone(),
            committed: false,
        });
    }

    for index in 0..staged_moves.len() {
        let temp = staged_moves[index].temp.clone();
        let target = staged_moves[index].target.clone();

        // Tahap 2: commit dari file sementara ke target final.
        if let Err(error) = fs::rename(&temp, &target) {
            let rollback_errors = rollback_all(&staged_moves);
            let rollback_message = if rollback_errors.is_empty() {
                String::new()
            } else {
                format!("\nRollback warning:\n- {}", rollback_errors.join("\n- "))
            };

            return Err(TransactionError {
                message: format!(
                    "Gagal commit rename {} -> {}: {}{}",
                    temp.display(),
                    target.display(),
                    error,
                    rollback_message
                ),
            });
        }

        staged_moves[index].committed = true;
    }

    Ok(())
}

fn rename_operation_from_paths(source: &Path, target: &Path) -> Result<RenameOperation, String> {
    let metadata = fs::metadata(source)
        .map_err(|error| format!("Gagal membaca metadata {}: {}", source.display(), error))?;

    let source_name = source
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .ok_or_else(|| format!("Nama file tidak valid: {}", source.display()))?;

    let target_name = target
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .ok_or_else(|| format!("Nama target tidak valid: {}", target.display()))?;

    let extension = source
        .extension()
        .map(|ext| ext.to_string_lossy().to_string())
        .unwrap_or_default();

    let modified_unix_ms = metadata
        .modified()
        .ok()
        .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
        .map(|duration| i64::try_from(duration.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0);

    Ok(RenameOperation {
        source_path: source.to_string_lossy().to_string(),
        target_path: target.to_string_lossy().to_string(),
        source_name,
        target_name,
        extension,
        size_bytes: metadata.len(),
        modified_unix_ms,
    })
}

fn generate_temp_path(source: &Path, index: usize) -> Result<PathBuf, String> {
    let parent = source
        .parent()
        .ok_or_else(|| format!("Path tidak valid: {}", source.display()))?;

    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| "Waktu sistem tidak valid.".to_string())?
        .as_nanos();

    for attempt in 0..1000usize {
        let candidate = parent.join(format!(
            ".smartfile_tmp_{}_{}_{}_{}",
            std::process::id(),
            nanos,
            index,
            attempt
        ));

        if !candidate.exists() {
            return Ok(candidate);
        }
    }

    Err(format!(
        "Gagal membuat nama sementara unik untuk {}",
        source.display()
    ))
}

fn rollback_all(staged_moves: &[StagedMove]) -> Vec<String> {
    let mut rollback_errors = Vec::new();

    for staged in staged_moves.iter().rev() {
        let from = if staged.committed {
            &staged.target
        } else {
            &staged.temp
        };

        if !from.exists() {
            continue;
        }

        if let Err(error) = fs::rename(from, &staged.original) {
            rollback_errors.push(format!(
                "Gagal rollback {} -> {}: {}",
                from.display(),
                staged.original.display(),
                error
            ));
        }
    }

    rollback_errors
}

fn main() {
    tauri::Builder::default()
        .manage(AppState {
            last_rename: Mutex::new(None),
        })
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            build_rename_plan,
            validate_rename_plan,
            apply_rename_plan,
            undo_last_rename
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
