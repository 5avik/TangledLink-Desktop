import { invoke } from "@tauri-apps/api/core";
import { english } from "./english";

let language = "ru";
try {
  if (localStorage.getItem("language") === "en") language = "en";
} catch { /* Use Russian when storage is unavailable. */ }

export function locale() { return language; }

export function t(text: string): string {
  return language === "en" ? english[text] ?? text : text;
}

// Capture only the original markup. Account IDs, server names and user input are never translated.
const staticText = new Map<Text, string>();
const walker = document.createTreeWalker(document.body, NodeFilter.SHOW_TEXT);
while (walker.nextNode()) {
  const node = walker.currentNode as Text;
  if (Object.hasOwn(english, node.data.trim())) staticText.set(node, node.data.trim());
}
const staticAttributes: Array<[Element, string, string]> = [];
for (const attribute of ["aria-label", "title", "placeholder"]) {
  for (const element of document.querySelectorAll("[" + attribute + "]")) {
    const original = element.getAttribute(attribute);
    if (original && Object.hasOwn(english, original)) staticAttributes.push([element, attribute, original]);
  }
}

function renderLanguage() {
  document.documentElement.lang = language;
  void invoke("set_language", { english: language === "en" }).catch(error => {
    document.querySelector("#preferences-message")!.textContent = String(error);
  });
  for (const [node, original] of staticText) node.data = t(original);
  for (const [element, attribute, original] of staticAttributes) element.setAttribute(attribute, t(original));
}

const selector = document.querySelector<HTMLSelectElement>("#language")!;
selector.value = language;
selector.addEventListener("change", () => {
  language = selector.value === "en" ? "en" : "ru";
  try { localStorage.setItem("language", language); } catch { /* Apply for this window. */ }
  renderLanguage();
  document.dispatchEvent(new Event("language-changed"));
});
renderLanguage();
