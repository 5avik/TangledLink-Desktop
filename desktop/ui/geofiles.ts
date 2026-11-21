import { invoke } from "@tauri-apps/api/core";
import { locale, t } from "./i18n";

type FileInfo = { size: number | null; modified: number | null };
type Status = { custom: boolean; files: [FileInfo, FileInfo] };

const section = document.querySelector<HTMLDetailsElement>("#geofiles")!;
const source = document.querySelector<HTMLSelectElement>("#geo-source")!;
const update = document.querySelector<HTMLButtonElement>("#geo-update")!;
const reset = document.querySelector<HTMLButtonElement>("#geo-reset")!;
const origin = document.querySelector("#geofiles-origin")!;
const message = document.querySelector("#geofiles-message")!;
const rows = [document.querySelector("#geoip-info")!, document.querySelector("#geosite-info")!];
let status: Status | undefined;
let busy = false;
let feedback = "";

try {
  const saved = localStorage.getItem("geo-source");
  if (saved !== null && ["0", "1", "2"].includes(saved)) source.value = saved;
} catch { /* Use the default source. */ }

source.addEventListener("change", () => {
  try { localStorage.setItem("geo-source", source.value); } catch { /* Apply for this window. */ }
});

function render() {
  const number = new Intl.NumberFormat(locale(), { style: "unit", unit: "megabyte", maximumFractionDigits: 2 });
  const date = new Intl.DateTimeFormat(locale(), { dateStyle: "medium", timeStyle: "short" });
  status?.files.forEach(({ size, modified }, index) => {
    const details = size === null ? t("Файл недоступен") : number.format(size / 1_000_000);
    rows[index].textContent = modified === null ? details : details + " · " + date.format(modified * 1000);
  });
  origin.textContent = status ? t(status.custom ? "Загруженные файлы" : "Файлы из пакета") : "";
  message.textContent = t(feedback);
  source.disabled = busy;
  update.disabled = busy;
  reset.disabled = busy || status?.custom === false;
}

async function perform(action: "status" | "update" | "reset") {
  if (busy) return;
  busy = true;
  feedback = action === "update" ? "Загрузка и проверка геофайлов…" : "";
  render();
  try {
    if (action === "update") {
      status = await invoke("update_geofiles", { source: Number(source.value) });
      feedback = "Геофайлы обновлены. Применятся при следующем подключении.";
    } else if (action === "reset") {
      status = await invoke("reset_geofiles");
      feedback = "Файлы из пакета восстановлены. Применятся при следующем подключении.";
    } else {
      status = await invoke("geofiles_status");
    }
  } catch (error) {
    feedback = String(error);
  } finally {
    busy = false;
    render();
  }
}

section.addEventListener("toggle", () => {
  if (section.open) void perform("status");
});
update.addEventListener("click", () => { void perform("update"); });
reset.addEventListener("click", () => { void perform("reset"); });
document.addEventListener("language-changed", render);
render();
