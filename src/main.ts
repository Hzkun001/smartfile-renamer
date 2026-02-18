import "./style.css";
import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";

type SortBy = "name" | "modified_time" | "size";
type ToastType = "success" | "error";

type RenameOperation = {
  source_path: string;
  target_path: string;
  source_name: string;
  target_name: string;
  extension: string;
  size_bytes: number;
  modified_unix_ms: number;
};

type RenamePlan = {
  operations: RenameOperation[];
};

type ConflictDetail = {
  source_path: string;
  target_path: string;
  message: string;
  kind: string;
};

type ValidateRenameResult = {
  total_files: number;
  safe_files: number;
  conflict_files: number;
  conflicts: ConflictDetail[];
};

type ApplyRenameResult = {
  dry_run: boolean;
  total_files: number;
  renamed_files: number;
  skipped_files: number;
  conflict_files: number;
  conflicts: ConflictDetail[];
  rolled_back: boolean;
  sample_targets: string[];
  message: string;
};

type UndoRenameResult = {
  restored_files: number;
  restored_paths: string[];
  message: string;
};

function requireElement<T extends Element>(selector: string): T {
  const element = document.querySelector<T>(selector);
  if (!element) {
    throw new Error(`Elemen tidak ditemukan: ${selector}`);
  }
  return element;
}

const pickFilesButton = requireElement<HTMLButtonElement>("#pick-files");
const pickFolderButton = requireElement<HTMLButtonElement>("#pick-folder");
const clearListButton = requireElement<HTMLButtonElement>("#clear-list");
const refreshPreviewButton = requireElement<HTMLButtonElement>("#refresh-preview");
const applyRenameButton = requireElement<HTMLButtonElement>("#apply-rename");
const undoRenameButton = requireElement<HTMLButtonElement>("#undo-rename");

const prefixInput = requireElement<HTMLInputElement>("#prefix");
const startNumberInput = requireElement<HTMLInputElement>("#start-number");
const paddingInput = requireElement<HTMLInputElement>("#padding");
const suffixInput = requireElement<HTMLInputElement>("#suffix");
const sortingSelect = requireElement<HTMLSelectElement>("#sorting");
const keepExtensionCheckbox = requireElement<HTMLInputElement>("#keep-extension");
const dryRunCheckbox = requireElement<HTMLInputElement>("#dry-run");

const fileDropZone = requireElement<HTMLDivElement>("#file-drop-zone");
const fileListBody = requireElement<HTMLTableSectionElement>("#file-list-body");
const previewBody = requireElement<HTMLTableSectionElement>("#preview-body");
const globalError = requireElement<HTMLParagraphElement>("#global-error");
const progressContainer = requireElement<HTMLDivElement>("#progress");
const progressText = requireElement<HTMLSpanElement>("#progress-text");
const summaryTotal = requireElement<HTMLElement>("#summary-total");
const summaryConflicts = requireElement<HTMLElement>("#summary-conflicts");
const summarySafe = requireElement<HTMLElement>("#summary-safe");
const toastContainer = requireElement<HTMLDivElement>("#toast-container");

const confirmOverlay = requireElement<HTMLDivElement>("#confirm-overlay");
const confirmDescription = requireElement<HTMLParagraphElement>("#confirm-description");
const confirmSamples = requireElement<HTMLUListElement>("#confirm-samples");
const confirmCancelButton = requireElement<HTMLButtonElement>("#confirm-cancel");
const confirmApplyButton = requireElement<HTMLButtonElement>("#confirm-apply");

let selectedInputs: string[] = [];
let activePlan: RenamePlan | null = null;
let activeValidation: ValidateRenameResult | null = null;
let isApplying = false;

function withTimeout<T>(promise: Promise<T>, timeoutMs: number, message: string): Promise<T> {
  return new Promise<T>((resolve, reject) => {
    const timer = window.setTimeout(() => {
      reject(new Error(message));
    }, timeoutMs);

    promise
      .then((value) => {
        window.clearTimeout(timer);
        resolve(value);
      })
      .catch((error) => {
        window.clearTimeout(timer);
        reject(error);
      });
  });
}

function showToast(message: string, type: ToastType): void {
  const toast = document.createElement("div");
  toast.className = `toast ${type}`;
  toast.textContent = message;
  toastContainer.appendChild(toast);

  window.setTimeout(() => {
    toast.remove();
  }, 3500);
}

function showError(message: string): void {
  globalError.textContent = message;
}

function clearError(): void {
  globalError.textContent = "";
}

function setProgress(visible: boolean, text = "Menerapkan rename..."): void {
  progressContainer.hidden = !visible;
  progressText.textContent = text;
}

function updateActionState(): void {
  const hasPlan = activePlan !== null;
  const hasConflicts = (activeValidation?.conflict_files ?? 0) > 0;
  const hasFiles = (activePlan?.operations.length ?? 0) > 0;

  applyRenameButton.disabled = !hasPlan || !hasFiles || hasConflicts || isApplying;
  refreshPreviewButton.disabled = !hasFiles || isApplying;
  clearListButton.disabled = selectedInputs.length === 0 || isApplying;
  pickFilesButton.disabled = isApplying;
  pickFolderButton.disabled = isApplying;
  undoRenameButton.disabled = isApplying;
}

function formatBytes(bytes: number): string {
  if (bytes < 1024) {
    return `${bytes} B`;
  }
  const units = ["KB", "MB", "GB", "TB"];
  let value = bytes;
  let unitIndex = -1;

  while (value >= 1024 && unitIndex < units.length - 1) {
    value /= 1024;
    unitIndex += 1;
  }

  return `${value.toFixed(1)} ${units[unitIndex]}`;
}

function formatTimestamp(unixMs: number): string {
  if (unixMs <= 0) {
    return "-";
  }
  const date = new Date(unixMs);
  return date.toLocaleString("id-ID");
}

function clearTables(): void {
  fileListBody.innerHTML = "";
  previewBody.innerHTML = "";
  summaryTotal.textContent = "0";
  summaryConflicts.textContent = "0";
  summarySafe.textContent = "0";
}

function updateSummary(validation: ValidateRenameResult): void {
  summaryTotal.textContent = String(validation.total_files);
  summaryConflicts.textContent = String(validation.conflict_files);
  summarySafe.textContent = String(validation.safe_files);
}

function conflictMapBySource(validation: ValidateRenameResult): Map<string, string[]> {
  const map = new Map<string, string[]>();

  for (const conflict of validation.conflicts) {
    const list = map.get(conflict.source_path) ?? [];
    list.push(conflict.message);
    map.set(conflict.source_path, list);
  }

  return map;
}

function renderFileList(plan: RenamePlan, validation: ValidateRenameResult): void {
  fileListBody.innerHTML = "";
  const conflictMap = conflictMapBySource(validation);

  for (const operation of plan.operations) {
    const row = document.createElement("tr");
    const conflicts = conflictMap.get(operation.source_path) ?? [];

    if (conflicts.length > 0) {
      row.classList.add("conflict-row");
    }

    const nameCell = document.createElement("td");
    nameCell.textContent = operation.source_name;

    const extCell = document.createElement("td");
    extCell.textContent = operation.extension || "-";

    const sizeCell = document.createElement("td");
    sizeCell.textContent = formatBytes(operation.size_bytes);

    const modifiedCell = document.createElement("td");
    modifiedCell.textContent = formatTimestamp(operation.modified_unix_ms);

    row.append(nameCell, extCell, sizeCell, modifiedCell);
    fileListBody.appendChild(row);
  }
}

function renderPreview(plan: RenamePlan, validation: ValidateRenameResult): void {
  previewBody.innerHTML = "";
  const conflictMap = conflictMapBySource(validation);

  for (const operation of plan.operations) {
    const row = document.createElement("tr");
    const conflicts = conflictMap.get(operation.source_path) ?? [];

    if (conflicts.length > 0) {
      row.classList.add("conflict-row");
    }

    const beforeCell = document.createElement("td");
    beforeCell.textContent = operation.source_name;

    const afterCell = document.createElement("td");
    afterCell.textContent = operation.target_name;

    if (conflicts.length > 0) {
      const warning = document.createElement("span");
      warning.className = "warning-text";
      warning.textContent = `⚠ ${conflicts[0]}`;
      afterCell.appendChild(document.createElement("br"));
      afterCell.appendChild(warning);
    }

    row.append(beforeCell, afterCell);
    previewBody.appendChild(row);
  }
}

function parseNumberInput(input: HTMLInputElement, fallback: number, min: number): number {
  const parsed = Number.parseInt(input.value, 10);
  if (!Number.isFinite(parsed) || parsed < min) {
    return fallback;
  }
  return parsed;
}

function buildSettingsRequest() {
  return {
    prefix: prefixInput.value,
    start_number: parseNumberInput(startNumberInput, 1, 1),
    padding: parseNumberInput(paddingInput, 0, 0),
    suffix: suffixInput.value,
    keep_extension: keepExtensionCheckbox.checked,
    sort_by: sortingSelect.value as SortBy
  };
}

function setUniqueInputPaths(nextPaths: string[]): void {
  const merged = new Set<string>(selectedInputs);
  for (const path of nextPaths) {
    if (typeof path === "string" && path.trim().length > 0) {
      merged.add(path);
    }
  }

  selectedInputs = Array.from(merged);
}

async function refreshPreview(): Promise<void> {
  clearError();

  if (selectedInputs.length === 0) {
    activePlan = null;
    activeValidation = null;
    clearTables();
    updateActionState();
    return;
  }

  try {
    const plan = await withTimeout(
      invoke<RenamePlan>("build_rename_plan", {
        request: {
          file_paths: selectedInputs,
          ...buildSettingsRequest()
        }
      }),
      30000,
      "Timeout saat build preview. Coba kurangi jumlah file atau ulangi."
    );

    const validation = await withTimeout(
      invoke<ValidateRenameResult>("validate_rename_plan", {
        request: {
          operations: plan.operations
        }
      }),
      30000,
      "Timeout saat validasi rename."
    );

    activePlan = plan;
    activeValidation = validation;
    renderFileList(plan, validation);
    renderPreview(plan, validation);
    updateSummary(validation);
    updateActionState();
  } catch (error) {
    activePlan = null;
    activeValidation = null;
    clearTables();
    updateActionState();
    showError(String(error));
  }
}

async function pickFiles(): Promise<void> {
  const picked = await open({
    multiple: true,
    directory: false,
    title: "Pilih file"
  });

  if (!picked) {
    return;
  }

  const values = Array.isArray(picked) ? picked : [picked];
  setUniqueInputPaths(values);
  await refreshPreview();
}

async function pickFolder(): Promise<void> {
  const picked = await open({
    multiple: false,
    directory: true,
    title: "Pilih folder"
  });

  if (!picked || Array.isArray(picked)) {
    return;
  }

  setUniqueInputPaths([picked]);
  await refreshPreview();
}

function openConfirmModal(): void {
  if (!activePlan) {
    return;
  }

  const dryRun = dryRunCheckbox.checked;
  const sample = activePlan.operations.slice(0, 3).map((item) => item.target_name);

  confirmDescription.textContent = dryRun
    ? `Dry run untuk ${activePlan.operations.length} file. Tidak ada file yang akan diubah.`
    : `Rename akan diterapkan ke ${activePlan.operations.length} file.`;

  confirmSamples.innerHTML = "";
  for (const name of sample) {
    const li = document.createElement("li");
    li.textContent = name;
    confirmSamples.appendChild(li);
  }

  confirmCancelButton.disabled = false;
  confirmApplyButton.disabled = false;
  confirmOverlay.hidden = false;
  confirmApplyButton.focus();
}

function closeConfirmModal(): void {
  confirmOverlay.hidden = true;
}

async function executeApply(): Promise<void> {
  if (!activePlan || !activeValidation) {
    return;
  }

  isApplying = true;
  setProgress(true, dryRunCheckbox.checked ? "Menjalankan dry run..." : "Menerapkan rename...");
  updateActionState();

  let result: ApplyRenameResult | null = null;

  try {
    result = await withTimeout(
      invoke<ApplyRenameResult>("apply_rename_plan", {
        request: {
          operations: activePlan.operations,
          dry_run: dryRunCheckbox.checked
        }
      }),
      180000,
      "Penerapan rename terlalu lama. Proses dihentikan di UI, cek status file lalu refresh."
    );
  } catch (error) {
    showToast(String(error), "error");
  } finally {
    isApplying = false;
    setProgress(false);
    updateActionState();
  }

  if (!result) {
    return;
  }

  if (result.conflict_files > 0) {
    showToast(`Rename diblokir. ${result.conflict_files} file konflik.`, "error");
  } else if (result.dry_run) {
    showToast(result.message, "success");
  } else {
    selectedInputs = activePlan.operations.map((item) => item.target_path);
    showToast(`Rename berhasil: ${result.renamed_files} file.`, "success");
  }

  await refreshPreview();
}

async function runUndo(): Promise<void> {
  isApplying = true;
  setProgress(true, "Menjalankan undo...");
  updateActionState();

  let result: UndoRenameResult | null = null;

  try {
    result = await withTimeout(
      invoke<UndoRenameResult>("undo_last_rename"),
      120000,
      "Undo terlalu lama. Proses dihentikan di UI, cek status file lalu refresh."
    );
  } catch (error) {
    showToast(String(error), "error");
  } finally {
    isApplying = false;
    setProgress(false);
    updateActionState();
  }

  if (!result) {
    return;
  }

  selectedInputs = result.restored_paths;
  showToast(`Undo berhasil: ${result.restored_files} file.`, "success");
  await refreshPreview();
}

function clearList(): void {
  selectedInputs = [];
  activePlan = null;
  activeValidation = null;
  clearError();
  clearTables();
  updateActionState();
}

function extractPathsFromDragEvent(event: DragEvent): string[] {
  const files = event.dataTransfer?.files;
  if (!files) {
    return [];
  }

  const paths: string[] = [];
  for (const file of Array.from(files)) {
    const path = (file as File & { path?: string }).path;
    if (typeof path === "string" && path.length > 0) {
      paths.push(path);
    }
  }

  return paths;
}

function setupDragAndDrop(): void {
  fileDropZone.addEventListener("dragover", (event) => {
    event.preventDefault();
    fileDropZone.classList.add("drag-over");
  });

  fileDropZone.addEventListener("dragleave", () => {
    fileDropZone.classList.remove("drag-over");
  });

  fileDropZone.addEventListener("drop", async (event) => {
    event.preventDefault();
    fileDropZone.classList.remove("drag-over");

    const paths = extractPathsFromDragEvent(event);
    if (paths.length === 0) {
      showToast("Drag & drop dari OS tidak terbaca. Gunakan Pick Files.", "error");
      return;
    }

    setUniqueInputPaths(paths);
    await refreshPreview();
  });

  // Event bawaan Tauri untuk file drop dari OS.
  window.addEventListener(
    "tauri://file-drop",
    (async (event: Event) => {
      const payload = (event as Event & { payload?: unknown }).payload;
      const paths = Array.isArray(payload)
        ? payload.filter((item): item is string => typeof item === "string")
        : [];

      if (paths.length === 0) {
        return;
      }

      fileDropZone.classList.remove("drag-over");
      setUniqueInputPaths(paths);
      await refreshPreview();
    }) as EventListener
  );

  window.addEventListener(
    "tauri://file-drop-hover",
    (() => {
      fileDropZone.classList.add("drag-over");
    }) as EventListener
  );

  window.addEventListener(
    "tauri://file-drop-cancelled",
    (() => {
      fileDropZone.classList.remove("drag-over");
    }) as EventListener
  );
}

pickFilesButton.addEventListener("click", async () => {
  clearError();
  await pickFiles();
});

pickFolderButton.addEventListener("click", async () => {
  clearError();
  await pickFolder();
});

clearListButton.addEventListener("click", () => {
  clearList();
});

refreshPreviewButton.addEventListener("click", async () => {
  await refreshPreview();
});

applyRenameButton.addEventListener("click", () => {
  if (!activePlan || !activeValidation) {
    showToast("Preview belum tersedia.", "error");
    return;
  }

  openConfirmModal();
});

undoRenameButton.addEventListener("click", async () => {
  await runUndo();
});

confirmCancelButton.addEventListener("click", () => {
  closeConfirmModal();
});

confirmApplyButton.addEventListener("click", async () => {
  closeConfirmModal();
  await executeApply();
});

confirmOverlay.addEventListener("click", (event) => {
  if (event.target === confirmOverlay) {
    closeConfirmModal();
  }
});

window.addEventListener("keydown", (event) => {
  if (event.key === "Escape" && !confirmOverlay.hidden) {
    closeConfirmModal();
  }
});

[prefixInput, startNumberInput, paddingInput, suffixInput, sortingSelect, keepExtensionCheckbox].forEach(
  (element) => {
    element.addEventListener("change", async () => {
      if (selectedInputs.length > 0) {
        await refreshPreview();
      }
    });
  }
);

setupDragAndDrop();
updateActionState();
