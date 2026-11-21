import { t } from "./i18n";

export function saveStatus(selector: string) {
  const status = document.querySelector<HTMLElement>(selector)!;
  let timeout: number | undefined;

  function clear() {
    window.clearTimeout(timeout);
    timeout = undefined;
    status.textContent = "";
  }

  document.addEventListener("language-changed", () => {
    if (timeout !== undefined) status.textContent = t("Сохранено");
  });

  return {
    clear,
    show() {
      clear();
      status.textContent = t("Сохранено");
      timeout = window.setTimeout(clear, 3000);
    },
  };
}
