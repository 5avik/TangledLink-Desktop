import { t } from "./i18n";
import { saveStatus } from "./save-status";
import { invoke } from "@tauri-apps/api/core";
type Preferences = { appearance: string; closeToTray: boolean; };
const defaults: Preferences = { appearance: "dark", closeToTray: true };
let preferences = defaults;
try {
  const saved = JSON.parse(localStorage.getItem("preferences") ?? "{}");
  preferences = {
    appearance: ["dark", "light", "system"].includes(saved.appearance) ? saved.appearance : "dark",
    closeToTray: typeof saved.closeToTray === "boolean" ? saved.closeToTray : true,
  };
} catch { /* Use defaults when stored preferences cannot be read. */ }

const appearance = document.querySelector<HTMLSelectElement>("#appearance")!;
const closeToTray = document.querySelector<HTMLInputElement>("#close-to-tray")!;
const system = matchMedia("(prefers-color-scheme: light)");
function apply() {
  document.documentElement.dataset.theme = preferences.appearance === "system"
    ? (system.matches ? "light" : "dark") : preferences.appearance;
}
function save() {
  preferences = { appearance: appearance.value, closeToTray: closeToTray.checked };
  apply();
  void invoke("set_close_to_tray", { enabled: preferences.closeToTray });
  try {
    localStorage.setItem("preferences", JSON.stringify(preferences));
  } catch {
    document.querySelector("#preferences-message")!.textContent = t("Не удалось сохранить настройки");
  }
}
appearance.value = preferences.appearance;
closeToTray.checked = preferences.closeToTray;
appearance.addEventListener("change", save);
closeToTray.addEventListener("change", save);
system.addEventListener("change", apply);
apply();
void invoke("set_close_to_tray", { enabled: preferences.closeToTray });

const mtuInput = document.querySelector<HTMLInputElement>("#vpn-mtu")!;
const feedback = document.querySelector("#preferences-message")!;
let mtu = 1500;
try {
  const stored = Number(localStorage.getItem("vpn-mtu"));
  if (Number.isInteger(stored) && stored >= 1280 && stored <= 1500) mtu = stored;
} catch { /* Use the default MTU. */ }
mtuInput.value = String(mtu);
void invoke("set_vpn_mtu", { mtu }).catch(error => { feedback.textContent = t(String(error)); });
mtuInput.addEventListener("change", async () => {
  if (!mtuInput.reportValidity() || !mtuInput.value) {
    mtuInput.value = String(mtu);
    return;
  }
  try {
    const next = mtuInput.valueAsNumber;
    await invoke("set_vpn_mtu", { mtu: next });
    mtu = next;
    localStorage.setItem("vpn-mtu", String(next));
    feedback.textContent = "";
  } catch (error) {
    feedback.textContent = t(String(error));
  }
});

const passwordForm = document.querySelector<HTMLFormElement>("#change-password")!;
const passwordSaved = saveStatus("#password-saved");
passwordForm.addEventListener("submit", async event => {
  event.preventDefault();
  passwordSaved.clear();
  const data = new FormData(passwordForm);
  const message = passwordForm.querySelector("p")!;
  const submit = passwordForm.querySelector("button")!;
  if (data.get("new") !== data.get("confirm")) {
    message.textContent = t("Пароли не совпадают.");
    return;
  }
  submit.disabled = true;
  try {
    await invoke("change_account_password", { current: data.get("current"), newPassword: data.get("new") });
    passwordForm.reset();
    message.textContent = "";
    passwordSaved.show();
  } catch (error) {
    message.textContent = t(String(error));
  } finally {
    submit.disabled = false;
  }
});

const localDirect = document.querySelector<HTMLInputElement>("#local-direct")!;
const mux = document.querySelector<HTMLInputElement>("#vpn-mux")!;
async function applyRouting(save: boolean) {
  try {
    if (!save) {
      localDirect.checked = localStorage.getItem("local-direct") === "true";
      mux.checked = localStorage.getItem("vpn-mux") === "true";
    }
    await invoke("set_linux_routing", { local: localDirect.checked, mux: mux.checked });
    if (save) {
      localStorage.setItem("local-direct", String(localDirect.checked));
      localStorage.setItem("vpn-mux", String(mux.checked));
      feedback.textContent = "";
    }
  } catch (error) {
    feedback.textContent = t(String(error));
  }
}
localDirect.addEventListener("change", () => { void applyRouting(true); });
mux.addEventListener("change", () => { void applyRouting(true); });
void applyRouting(false);

const mode = document.querySelector<HTMLSelectElement>("#connection-mode")!;
const ipv6 = document.querySelector<HTMLInputElement>("#vpn-ipv6")!;
const dns = document.querySelector<HTMLSelectElement>("#vpn-dns")!;
const domains = document.querySelector<HTMLTextAreaElement>("#direct-domains")!;
const ips = document.querySelector<HTMLTextAreaElement>("#direct-ips")!;
const concurrency = document.querySelector<HTMLInputElement>("#mux-concurrency")!;
const fragment = document.querySelector<HTMLInputElement>("#vpn-fragment")!;
const saveCore = document.querySelector<HTMLButtonElement>("#save-core-options")!;
const lines = (input: HTMLTextAreaElement) => input.value.split("\n").map(line => line.trim()).filter(Boolean);
function coreOptions() {
  return { proxy_only: mode.value === "proxy", split_apps: mode.value === "split", ipv6: ipv6.checked, use_google_dns: dns.value === "google",
    domains: lines(domains), ips: lines(ips), concurrency: concurrency.valueAsNumber, fragment: fragment.checked };
}
concurrency.value = "8";
try {
  const saved = JSON.parse(localStorage.getItem("core-options") ?? "{}");
  mode.value = saved.proxy_only ? "proxy" : saved.split_apps ? "split" : "tun";
  ipv6.checked = saved.ipv6 === true;
  dns.value = saved.use_google_dns ? "google" : "cloudflare";
  domains.value = Array.isArray(saved.domains) ? saved.domains.join("\n") : "";
  ips.value = Array.isArray(saved.ips) ? saved.ips.join("\n") : "";
  if (Number.isInteger(saved.concurrency)) concurrency.value = String(saved.concurrency);
  fragment.checked = saved.fragment === true;
} catch { /* Use defaults. */ }
void invoke("set_core_options", { options: coreOptions() }).catch(error => { feedback.textContent = t(String(error)); });
const coreSaved = saveStatus("#core-saved");
saveCore.addEventListener("click", async () => {
  coreSaved.clear();
  if (!concurrency.reportValidity()) return;
  saveCore.disabled = true;
  try {
    const options = coreOptions();
    await invoke("set_core_options", { options });
    localStorage.setItem("core-options", JSON.stringify(options));
    feedback.textContent = "";
    coreSaved.show();
  } catch (error) {
    feedback.textContent = t(String(error));
  } finally {
    saveCore.disabled = false;
  }
});
