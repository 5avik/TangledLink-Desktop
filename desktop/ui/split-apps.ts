import { invoke } from "@tauri-apps/api/core";
import { t } from "./i18n";

const programInput = document.querySelector<HTMLInputElement>("#split-program")!;
const add = document.querySelector<HTMLButtonElement>("#split-add")!;
const list = document.querySelector<HTMLUListElement>("#split-programs")!;
const message = document.querySelector("#split-message")!;
let programs: string[] = [];
let pending = false;
let feedback = "";

try {
  const saved: unknown = JSON.parse(localStorage.getItem("split-programs") ?? "[]");
  if (Array.isArray(saved)) programs = saved.filter((entry): entry is string => typeof entry === "string" && entry.startsWith("/"));
} catch { /* Start with an empty list. */ }

function render() {
  message.textContent = t(feedback);
  list.replaceChildren(...programs.map(program => {
    const row = document.createElement("li");
    const name = document.createElement("span");
    const launch = document.createElement("button");
    const remove = document.createElement("button");
    name.textContent = program;
    launch.type = remove.type = "button";
    launch.className = remove.className = "secondary";
    launch.textContent = t("Запустить");
    remove.textContent = t("Удалить");
    launch.disabled = pending;
    launch.onclick = async () => {
      pending = true;
      feedback = "";
      render();
      try {
        await invoke("launch_split_app", { program });
        feedback = "Запуск приложения запрошен.";
      } catch (error) {
        feedback = String(error);
      } finally {
        pending = false;
        render();
      }
    };
    remove.onclick = () => save(programs.filter(entry => entry !== program));
    row.append(name, launch, remove);
    return row;
  }));
}

function save(next: string[]) {
  try {
    localStorage.setItem("split-programs", JSON.stringify(next));
    programs = next;
    feedback = "";
  } catch {
    feedback = "Не удалось сохранить настройки";
  }
  render();
}

add.onclick = () => {
  const program = programInput.value.trim();
  if (!program.startsWith("/")) {
    feedback = "Укажи абсолютный путь к исполняемому файлу.";
    render();
    return;
  }
  if (!programs.includes(program)) save([...programs, program]);
  programInput.value = "";
};
programInput.addEventListener("keydown", event => {
  if (event.key === "Enter") add.click();
});
document.addEventListener("language-changed", render);
render();
