import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { t } from "./i18n";

type Update = { version: string; notes: string };
const dialog = document.querySelector<HTMLDialogElement>("#update-dialog")!;
const details = document.querySelector<HTMLElement>("#update-notes")!;
const version = document.querySelector("#update-version")!;
const progress = document.querySelector<HTMLProgressElement>("#update-progress")!;
const action = document.querySelector<HTMLButtonElement>("#update-install")!;
const later = document.querySelector<HTMLButtonElement>("#update-later")!;
const status = document.querySelector("#update-status")!;
const checkButton = document.querySelector<HTMLButtonElement>("#check-update")!;
const checkStatus = document.querySelector("#check-update-status")!;
let update: Update | null = null;
let busy = false;
let feedback = "";
let checkFeedback = "";

function render() {
  status.textContent = t(feedback);
  checkStatus.textContent = t(checkFeedback);
}
document.addEventListener("language-changed", render);

async function check(manual: boolean) {
  if (checkButton.disabled || dialog.open) return;
  checkButton.disabled = true;
  checkFeedback = "";
  render();
  try {
    update = await invoke("check_for_update");
    if (update) {
      version.textContent = update.version;
      details.textContent = update.notes;
      details.hidden = !update.notes.trim();
      progress.hidden = true;
      feedback = "";
      render();
      dialog.showModal();
    } else if (manual) {
      checkFeedback = "Подходящее обновление недоступно.";
    }
  } catch (error) {
    if (manual) checkFeedback = String(error);
  } finally {
    checkButton.disabled = false;
    render();
  }
}

action.addEventListener("click", async () => {
  if (!update || busy) return;
  busy = action.disabled = later.disabled = true;
  progress.max = 100;
  progress.value = 0;
  progress.hidden = false;
  feedback = "Скачиваем обновление…";
  render();
  try {
    await invoke("install_update", { version: update.version });
    feedback = "Обновление установлено. Перезапускаем приложение…";
  } catch (error) {
    feedback = String(error);
    busy = action.disabled = later.disabled = false;
    progress.hidden = true;
  }
  render();
});
later.addEventListener("click", () => dialog.close());
dialog.addEventListener("cancel", event => {
  if (busy) event.preventDefault();
});
checkButton.addEventListener("click", () => { void check(true); });
void Promise.allSettled([
  listen<{ received: number; total: number }>("update-progress", ({ payload }) => {
    if (busy && payload.total > 0) progress.value = (payload.received / payload.total) * 100;
  }),
  listen("update-installing", () => {
    if (!busy) return;
    progress.removeAttribute("value");
    feedback = "Устанавливаем обновление. Подтверди запрос администратора.";
    render();
  }),
]).then(() => { void check(false); });
