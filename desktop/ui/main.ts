import { setBillingAccount } from "./billing";
import "./updater";
import { saveStatus } from "./save-status";
import { t, locale } from "./i18n";
import "./preferences";
import "./geofiles";
import "./split-apps";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";

type Server = { name: string; transport: string };
type Usage = { used: number | null; limit: number | null; expires_at: number | null };
type Snapshot = { servers_deferred?: boolean; can_quit: boolean; checking_session: boolean; servers: Server[]; active: number | null; selected: number; warning: string | null; account: string | null; account_notice: string | null; subscribed: boolean | null; usage: Usage | null };

const serverTools = document.querySelector<HTMLDivElement>("#server-tools")!;
const search = document.querySelector<HTMLInputElement>("#server-search")!;
const measure = document.querySelector<HTMLButtonElement>("#measure-latency")!;
const sortLatency = document.querySelector<HTMLButtonElement>("#sort-latency")!;
const list = document.querySelector<HTMLDivElement>("#servers")!;
const status = document.querySelector<HTMLParagraphElement>("#status")!;
const activeName = document.querySelector<HTMLParagraphElement>("#active-name")!;
const switchServer = document.querySelector<HTMLButtonElement>("#switch-server")!;
const quitApp = document.querySelector<HTMLButtonElement>("#quit-app")!;
const toggle = document.querySelector<HTMLButtonElement>("#toggle")!;
const message = document.querySelector<HTMLParagraphElement>("#message")!;

const connection = document.querySelector<HTMLElement>(".connection")!;
const picker = document.querySelector<HTMLElement>("#server-picker")!;
const serverPanel = document.querySelector<HTMLElement>("#server-panel")!;
const serverTrigger = document.querySelector<HTMLButtonElement>("#server-trigger")!;
const selectedServer = document.querySelector("#selected-server")!;
const selectedProtocol = document.querySelector("#selected-protocol")!;
const infoServer = document.querySelector("#info-server")!;
const infoProtocol = document.querySelector("#info-protocol")!;
const infoLatency = document.querySelector("#info-latency")!;
const infoCore = document.querySelector("#info-core")!;
const accountName = document.querySelector("#account-name")!;
const accountDialog = document.querySelector<HTMLDialogElement>("#account")!;
const settingsDialog = document.querySelector<HTMLDialogElement>("#preferences-dialog")!;
const accountMessage = document.querySelector("#account-message")!;

const startup = document.querySelector<HTMLParagraphElement>("#startup")!;
const signIn = document.querySelector<HTMLElement>("#sign-in")!;
const dashboard = document.querySelector<HTMLDivElement>("#dashboard")!;
let ready = false;

const accountTitle = document.querySelector<HTMLElement>("#account-title")!;
const loginForm = document.querySelector<HTMLFormElement>("#login")!;
const accountId = document.querySelector<HTMLInputElement>("#account-id")!;
const password = document.querySelector<HTMLInputElement>("#account-password")!;
const code = document.querySelector<HTMLInputElement>("#account-code")!;
const challenge = document.querySelector<HTMLDivElement>("#challenge")!;
const accountNotice = document.querySelector<HTMLParagraphElement>("#account-notice")!;
const loginButton = document.querySelector<HTMLButtonElement>("#login-submit")!;
const refreshButtons = document.querySelectorAll<HTMLButtonElement>("#account-refresh, #subscription-refresh");
const logoutButton = document.querySelector<HTMLButtonElement>("#account-logout")!;

const authTitle = document.querySelector<HTMLHeadingElement>("#sign-in-title")!;
const registrationRules = document.querySelector<HTMLElement>("#registration-rules")!;
const registrationEmail = document.querySelector<HTMLDivElement>("#registration-email")!;
const accountEmail = document.querySelector<HTMLInputElement>("#account-email")!;
const authResend = document.querySelector<HTMLButtonElement>("#auth-resend")!;
const openRegister = document.querySelector<HTMLButtonElement>("#open-register")!;
const openRecovery = document.querySelector<HTMLButtonElement>("#open-recovery")!;
const backLogin = document.querySelector<HTMLButtonElement>("#back-login")!;
const recoverForm = document.querySelector<HTMLFormElement>("#recover")!;
const recoveryStart = document.querySelector<HTMLFieldSetElement>("#recovery-start")!;
const recoveryActivation = document.querySelector<HTMLFieldSetElement>("#recovery-activation")!;
const recoveryVerification = document.querySelector<HTMLFieldSetElement>("#recovery-verification")!;
const recoverySupport = document.querySelector<HTMLParagraphElement>("#recovery-support")!;
const recoveryDone = document.querySelector<HTMLParagraphElement>("#recovery-done")!;
const recoveryId = document.querySelector<HTMLInputElement>("#recovery-id")!;
const recoveryInvite = document.querySelector<HTMLInputElement>("#recovery-invite")!;
const recoveryEmail = document.querySelector<HTMLInputElement>("#recovery-email")!;
const recoveryCode = document.querySelector<HTMLInputElement>("#recovery-code")!;
const recoveryPassword = document.querySelector<HTMLInputElement>("#recovery-password")!;
const recoveryConfirm = document.querySelector<HTMLInputElement>("#recovery-confirm")!;
const recoveryResend = document.querySelector<HTMLButtonElement>("#recovery-resend")!;
const recoverySaved = saveStatus("#recovery-saved");
const recoverySubmit = document.querySelector<HTMLButtonElement>("#recovery-submit")!;

type AuthMode = "login" | "register" | "recover";
type RecoveryStep = "login" | "activation" | "code" | "support" | "done";
let authMode: AuthMode = "login";
let recoveryStep: RecoveryStep = "login";
let needsActivation = false;
let awaitingAuthCode = false;
let resendAfter = 0;
let resendTimer: number | undefined;

let current: Snapshot = { can_quit: false, checking_session: false, servers: [], active: null, selected: 0, warning: null, account: null, account_notice: null, subscribed: null, usage: null };

let latency: (number | null)[] = [];
let sortByLatency = false;
let pendingServer: number | null = null;
let busy = false;
let refreshing = false;
let connectionAction: "connect" | "disconnect" | "switch" | null = null;
let connectionError: string | null = null;

const stats = document.querySelector<HTMLElement>("#subscription-stats")!;
const subscriptionState = document.querySelector<HTMLParagraphElement>("#subscription-state")!;
const subscriptionUsage = document.querySelector<HTMLParagraphElement>("#subscription-usage")!;
const subscriptionExpiry = document.querySelector<HTMLParagraphElement>("#subscription-expiry")!;



function formatBytes(bytes: number) {
  const units = [t("Б"), t("КиБ"), t("МиБ"), t("ГиБ"), t("ТиБ"), t("ПиБ"), t("ЭиБ")];
  const index = bytes > 0 ? Math.min(Math.floor(Math.log(bytes) / Math.log(1024)), units.length - 1) : 0;
  return new Intl.NumberFormat(locale(), { maximumFractionDigits: 1 }).format(bytes / 1024 ** index) + " " + units[index];
}

function renderSubscriptionStats() {
  const { subscribed, usage } = current;
  let traffic = "";
  if (usage?.used != null) {
    traffic = t("Трафик ") + formatBytes(usage.used);
    if (usage.limit === 0) traffic += t(" · без лимита");
    else if (usage.limit !== null) traffic += t(" из ") + formatBytes(usage.limit);
  } else if (usage?.limit != null) {
    traffic = usage.limit === 0 ? t("Трафик без лимита") : t("Лимит трафика ") + formatBytes(usage.limit);
  }
  const expiry = usage?.expires_at ? new Date(usage.expires_at * 1000) : null;
  const remaining = expiry ? expiry.getTime() - Date.now() : NaN;
  const expired = remaining <= 0;
  const soon = subscribed === true && remaining > 0 && remaining <= 7 * 24 * 60 * 60 * 1000;
  stats.dataset.tone = expired || subscribed === false ? "expired" : soon ? "soon" : subscribed ? "active" : "unknown";
  subscriptionState.textContent = expired ? t("Подписка истекла") : soon ? t("Подписка скоро истекает")
    : subscribed == null ? "" : subscribed ? t("Подписка активна") : t("Подписка неактивна");
  subscriptionUsage.textContent = traffic;
  subscriptionExpiry.textContent = expiry && Number.isFinite(expiry.getTime())
    ? t("Действует до ") + new Intl.DateTimeFormat(locale(), { dateStyle: "medium", timeStyle: "short" }).format(expiry) : "";
  for (const field of [subscriptionState, subscriptionUsage, subscriptionExpiry]) {
    field.hidden = !field.textContent;
  }
  stats.hidden = current.account === null;
}

function renderAuth() {
  const registering = authMode === "register";
  const recovering = authMode === "recover";
  authTitle.textContent = registering ? t("Создать аккаунт") : recovering ? t("Восстановление пароля") : t("Вход в аккаунт");
  loginForm.hidden = recovering;
  recoverForm.hidden = !recovering;
  registrationRules.hidden = !registering || awaitingAuthCode;
  registrationEmail.hidden = !registering;
  accountEmail.required = registering;
  password.autocomplete = registering ? "new-password" : "current-password";
  challenge.hidden = !awaitingAuthCode;
  code.required = awaitingAuthCode;
  for (const input of [accountId, password, accountEmail]) input.readOnly = awaitingAuthCode || busy;
  openRegister.hidden = openRecovery.hidden = authMode !== "login";
  backLogin.hidden = authMode === "login" && !awaitingAuthCode;
  for (const button of [openRegister, openRecovery, backLogin, loginButton]) button.disabled = busy;
  loginButton.textContent = awaitingAuthCode ? t("Подтвердить код") : registering ? t("Создать аккаунт") : t("Войти");

  recoveryStart.hidden = recoveryStep !== "login";
  recoveryActivation.hidden = recoveryStep !== "activation";
  recoveryVerification.hidden = recoveryStep !== "code";
  for (const fieldset of [recoveryStart, recoveryActivation, recoveryVerification]) {
    fieldset.disabled = busy || fieldset.hidden;
  }
  recoverySupport.hidden = recoveryStep !== "support";
  recoveryDone.hidden = recoveryStep !== "done";
  recoverySubmit.textContent = recoveryStep === "done" || recoveryStep === "support" ? t("Вернуться ко входу")
    : recoveryStep === "code" ? t("Сохранить пароль") : t("Продолжить");
  recoverySubmit.disabled = busy;

  window.clearTimeout(resendTimer);
  const wait = Math.max(0, Math.ceil((resendAfter - Date.now()) / 1000));
  if (wait > 0 && (awaitingAuthCode || recoveryStep === "code") && !document.hidden) {
    resendTimer = window.setTimeout(renderAuth, 1000);
  }
  for (const button of [authResend, recoveryResend]) {
    button.disabled = busy || wait > 0;
    button.textContent = wait ? t("Повторить через ") + wait + t(" с") : t("Отправить код ещё раз");
  }
}

function renderControls() {
  renderSubscriptionStats();
  quitApp.hidden = !current.can_quit;
  document.querySelector<HTMLElement>("#linux-vpn-settings")!.hidden = !current.can_quit;
  quitApp.disabled = busy;
  const active = current.active !== null;
  const state = connectionAction ? "connecting" : active ? "connected" : connectionError ? "error" : "disconnected";
  connection.dataset.state = state;
  const countries = new Intl.DisplayNames(locale(), { type: "region", fallback: "none" });
  const activeServer = current.active === null ? undefined : current.servers[current.active];
  const selected = current.servers[pendingServer ?? current.active ?? current.selected];
  const activeLabel = activeServer ? serverLabel(activeServer, countries) : undefined;
  const selectedLabel = selected ? serverLabel(selected, countries) : undefined;
  const statusText = t(connectionAction === "disconnect" ? "Отключение…" : state === "connecting" ? "Подключение…"
    : state === "connected" ? "Защита активна" : state === "error" ? "Ошибка подключения" : "Не подключено");
  if (status.textContent !== statusText) status.textContent = statusText;
  activeName.textContent = state === "error" ? t(connectionError ?? "")
    : state === "connecting" ? t(connectionAction === "switch" ? "Переключаем сервер…" : "")
    : state === "connected" ? activeLabel?.name ?? "" : t("Нажми для подключения");
  toggle.setAttribute("aria-label", t(active ? "Отключить" : "Подключить"));
  toggle.disabled = busy || (!active && current.servers.length === 0);
  switchServer.hidden = !active || pendingServer === null || pendingServer === current.active;
  switchServer.disabled = busy;
  selectedServer.textContent = selectedLabel?.name ?? t("Выбери сервер");
  selectedProtocol.textContent = selectedLabel?.transport ?? "";
  infoServer.textContent = activeLabel?.name ?? "—";
  infoProtocol.textContent = activeLabel?.transport || "—";
  const measured = current.active === null ? null : latency[current.active];
  infoLatency.textContent = measured == null ? "—" : measured + t(" мс");
  infoCore.textContent = active ? t("Работает") : "—";
  const signed = current.account !== null;
  accountName.textContent = current.account ?? "";
  setBillingAccount(current.account);
  accountMessage.textContent = message.textContent;
  if (!signed && accountDialog.open) accountDialog.close();
  const wasHidden = signIn.hidden;
  const checking = !ready || current.checking_session;
  startup.hidden = !checking;
  signIn.hidden = checking || signed;
  dashboard.hidden = checking || !signed;
  renderAuth();
  if (wasHidden && !signIn.hidden) (authMode === "recover" ? recoveryId : accountId).focus();
  for (const button of refreshButtons) button.disabled = busy;
  logoutButton.disabled = busy;
  accountNotice.textContent = t(current.account_notice ?? "");
  serverTools.hidden = current.servers.length === 0;
  measure.disabled = busy || active;
  sortLatency.hidden = latency.length === 0;
  sortLatency.setAttribute("aria-pressed", String(sortByLatency));
  for (const radio of list.querySelectorAll<HTMLInputElement>("input")) {
    const index = Number(radio.value);
    radio.disabled = busy;
    radio.checked = index === (active ? pendingServer ?? current.active : current.selected);
  }
}

function serverLabel(server: Server, countries: Intl.DisplayNames) {
  const transport = server.transport === "raw" ? "TCP" : server.transport.toUpperCase();
  const suffix = server.name.match(/\s+(\S+)\s*$/u);
  const shortened = suffix?.[1].toUpperCase() === transport ? server.name.slice(0, suffix.index).trimEnd() : "";
  const name = t(shortened || server.name);
  const flag = name.match(/^(\p{Regional_Indicator}{2})(?!\p{Regional_Indicator})\s*/u);
  if (!flag) return { name, transport };

  const region = Array.from(flag[1], letter => String.fromCharCode(letter.codePointAt(0)! - 0x1f1e6 + 65)).join("");
  const country = countries.of(region);
  if (!country) return { name, transport };

  const description = name.slice(flag[0].length);
  return { name: flag[1] + " " + country + (description ? " " + description : ""), transport };
}

function renderServers() {
  list.replaceChildren();
  const query = search.value.trim().toLocaleLowerCase();
  const countries = new Intl.DisplayNames(locale(), { type: "region", fallback: "none" });
  const visible = current.servers
    .map((server, index) => ({ server, index, label: serverLabel(server, countries) }))
    .filter(({ server, label }) => [label.name, server.name, label.transport].some(
      text => text.toLocaleLowerCase().includes(query)
    ));
  if (sortByLatency) {
    const time = (index: number) => latency[index] ?? Number.MAX_SAFE_INTEGER;
    visible.sort((a, b) => time(a.index) - time(b.index));
  }
  if (!visible.length) {
    const empty = document.createElement("p");
    empty.className = "empty";
    empty.textContent = current.servers.length ? t("По запросу ничего не найдено") : t("Нет доступных серверов");
    list.append(empty);
    return;
  }
  visible.forEach(({index, label}) => {
    const row = document.createElement("label");
    row.className = "server";
    const radio = document.createElement("input");
    radio.type = "radio";
    radio.name = "server";
    radio.value = String(index);
    radio.checked = index === current.selected;
    radio.addEventListener("change", () => {
      setServerPanel(false, true);
      if (current.active !== null) {
        pendingServer = index === current.active ? null : index;
        renderControls();
      } else {
        void act(() => invoke("select_server", { index }), () => {});
      }
    });
    const name = document.createElement("span");
    name.className = "server-name";
    name.textContent = label.name;
    const transport = document.createElement("span");
    transport.className = "transport";
    transport.textContent = label.transport || "—";
    row.append(radio, name, transport);
    const delay = document.createElement("span");
    delay.className = "transport delay";
    delay.textContent = latency[index] == null ? "—" : latency[index] + t(" мс");
    if (latency[index] === null) delay.title = t("Нет ответа");
    row.append(delay);
    list.append(row);
  });
}

let operationRevision = 0;

async function act(operation: () => Promise<Snapshot>, result: (snapshot: Snapshot) => void, pendingMessage = t("Выполняется…")) {
  if (busy) return;
  operationRevision++;
  busy = true;
  message.textContent = pendingMessage;
  renderControls();
  try {
    const snapshot = await operation();
    if (snapshot.servers !== current.servers && (
      snapshot.account !== current.account || JSON.stringify(snapshot.servers) !== JSON.stringify(current.servers)
    )) clearLatency();
    current = snapshot;
    message.textContent = "";
    result(current);
  } catch (error) {
    try { current = await invoke<Snapshot>("status"); } catch { /* The next status poll will retry. */ }
    const detail = String(error);
    if (connectionAction && current.active === null) connectionError = detail;
    message.textContent = connectionError === detail ? "" : t(detail);
    renderServers();
  } finally {
    connectionAction = null;
    busy = false;
    renderControls();
  }
}

function clearLatency() {
  latency = [];
  sortByLatency = false;
}

search.addEventListener("input", () => {
  renderServers();
  renderControls();
});

sortLatency.addEventListener("click", () => {
  sortByLatency = !sortByLatency;
  renderServers();
  renderControls();
});

measure.addEventListener("click", () => {
  void act(async () => {
    latency = await invoke<(number | null)[]>("measure_latency");
    return current;
  }, () => {
    renderServers();
    message.textContent = t("Проверены TCP-подключения к серверам");
  }, t("Проверяем задержку TCP…"));
});

toggle.addEventListener("click", () => {
  if (busy) return;
  const command = current.active === null ? "connect" : "disconnect";
  connectionAction = command;
  connectionError = null;
  pendingServer = null;
  void act(() => invoke(command, { index: current.selected }), renderServers);
});

switchServer.addEventListener("click", () => {
  if (busy || pendingServer === null || current.active === null) return;
  connectionAction = "switch";
  connectionError = null;
  const index = pendingServer;
  pendingServer = null;
  void act(() => invoke("connect", { index }), () => {
    message.textContent = t("Сервер переключён");
  }, t("Переключаем сервер…"));
});

function openAuthMode(mode: AuthMode, username = "") {
  authMode = mode;
  awaitingAuthCode = false;
  recoverySaved.clear();
  recoveryStep = "login";
  needsActivation = false;
  resendAfter = 0;
  loginForm.reset();
  recoverForm.reset();
  accountId.value = username;
  recoveryId.value = username;
  message.textContent = "";
  renderAuth();
  if (mode === "recover") recoveryId.focus();
  else accountId.focus();
}

openRegister.addEventListener("click", () => openAuthMode("register", accountId.value));
openRecovery.addEventListener("click", () => openAuthMode("recover", accountId.value));
backLogin.addEventListener("click", () => openAuthMode("login", authMode === "recover" ? recoveryId.value : accountId.value));

function submitAuthentication(resend = false) {
  let challengeResult: { email_required: boolean; retry_after_seconds: number; snapshot: Snapshot | null };
  void act(async () => {
    challengeResult = await invoke("login_account", {
      id: accountId.value, password: password.value,
      emailCode: awaitingAuthCode && !resend ? code.value : null,
      registrationEmail: authMode === "register" ? accountEmail.value : null,
    });
    return challengeResult.snapshot ?? current;
  }, () => {
    if (challengeResult.email_required) {
      awaitingAuthCode = true;
      resendAfter = Date.now() + challengeResult.retry_after_seconds * 1000;
      code.value = "";
      message.textContent = t("Введи шесть цифр из письма. Код действует 10 минут.");
      renderAuth();
      code.focus();
      return;
    }
    openAuthMode("login");
    renderServers();
  });
}

loginForm.addEventListener("submit", (event) => {
  event.preventDefault();
  submitAuthentication();
});
authResend.addEventListener("click", () => submitAuthentication(true));

function submitRecovery(resend = false) {
  const confirming = recoveryStep === "code" && !resend;
  if (confirming && recoveryPassword.value !== recoveryConfirm.value) {
    message.textContent = t("Пароли не совпадают.");
    recoveryConfirm.focus();
    return;
  }
  let result: { step: RecoveryStep; retry_after_seconds: number };
  void act(async () => {
    result = await invoke("recover_account", {
      id: recoveryId.value,
      email: needsActivation ? recoveryEmail.value : null,
      inviteCode: needsActivation ? recoveryInvite.value : null,
      emailCode: confirming ? recoveryCode.value : null,
      password: confirming ? recoveryPassword.value : null,
    });
    return current;
  }, () => {
    recoveryStep = result.step;
    if (recoveryStep === "activation") {
      needsActivation = true;
    }
    if (recoveryStep === "code") {
      recoveryCode.value = "";
      resendAfter = Date.now() + result.retry_after_seconds * 1000;
    }
    if (recoveryStep === "done") {
      if (confirming) recoverySaved.show();
      recoveryCode.value = recoveryPassword.value = recoveryConfirm.value = recoveryInvite.value = "";
    }
    renderAuth();
    if (recoveryStep === "activation") recoveryInvite.focus();
    if (recoveryStep === "code") recoveryCode.focus();
    if (recoveryStep === "support" || recoveryStep === "done") recoverySubmit.focus();
  });
}

recoverForm.addEventListener("submit", (event) => {
  event.preventDefault();
  if (recoveryStep === "support" || recoveryStep === "done") {
    openAuthMode("login", recoveryId.value);
  } else {
    submitRecovery();
  }
});
recoveryResend.addEventListener("click", () => submitRecovery(true));

for (const button of refreshButtons) {
  button.addEventListener("click", () => {
    void act(() => invoke("refresh_account"), snapshot => {
      renderServers();
      if (snapshot.account) message.textContent = t(snapshot.servers_deferred
        ? "Подписка обновлена. Новый список серверов применится после отключения VPN."
        : "Подписка обновлена.");
    }, t("Обновляем подписку…"));
  });
}

logoutButton.addEventListener("click", () => {
  void act(() => invoke("logout_account"), (snapshot) => {
    renderServers();
    message.textContent = t(snapshot.warning ?? "Ты вышел из аккаунта");
  });
});

async function refresh() {
  if (document.hidden || busy || refreshing) return;
  const revision = operationRevision;
  refreshing = true;
  try {
    const snapshot = await invoke<Snapshot>("status");
    if (busy || revision !== operationRevision) return;
    ready = true;
    const changed = snapshot.account !== current.account || JSON.stringify(snapshot.servers) !== JSON.stringify(current.servers);
    if (current.active !== snapshot.active) pendingServer = null;
    if (current.active !== null && snapshot.active === null) {
      connectionError = snapshot.warning ?? "Туннель остановился";
    }
    if (current.warning && !snapshot.warning && message.textContent === t(current.warning)) {
      message.textContent = "";
    }
    if (snapshot.active !== null || snapshot.account !== current.account) connectionError = null;
    current = snapshot;
    if (changed) { clearLatency(); renderServers(); }
    if (snapshot.warning) message.textContent = t(snapshot.warning);
    renderControls();
  } catch (error) {
    if (!busy && revision === operationRevision && ready) message.textContent = t(String(error));
  } finally {
    refreshing = false;
  }
}

function setServerPanel(open: boolean, restoreFocus = false) {
  serverPanel.hidden = !open;
  serverTrigger.setAttribute("aria-expanded", String(open));
  if (open) {
    if (!serverTools.hidden) search.focus();
    else serverTrigger.focus();
  } else if (restoreFocus) {
    serverTrigger.focus();
  }
}

serverTrigger.addEventListener("click", () => setServerPanel(serverPanel.hidden));
document.addEventListener("pointerdown", event => {
  if (!serverPanel.hidden && event.target instanceof Node && !picker.contains(event.target)) setServerPanel(false);
});
picker.addEventListener("focusout", event => {
  if (event.relatedTarget instanceof Node && !picker.contains(event.relatedTarget)) setServerPanel(false);
});
picker.addEventListener("keydown", event => {
  if (event.key === "Escape" && !serverPanel.hidden) {
    event.preventDefault();
    setServerPanel(false, true);
  } else if (event.key === "ArrowDown" || event.key === "ArrowUp") {
    if (!(event.target === search || event.target === serverTrigger || event.target instanceof HTMLInputElement)) return;
    event.preventDefault();
    if (serverPanel.hidden) setServerPanel(true);
    const radios = Array.from(list.querySelectorAll<HTMLInputElement>("input:not(:disabled)"));
    if (!radios.length) return;
    const index = radios.indexOf(document.activeElement as HTMLInputElement);
    const next = event.key === "ArrowDown" ? index + 1 : index < 0 ? radios.length - 1 : index - 1;
    radios[(next + radios.length) % radios.length].focus();
  } else if (event.key === "Enter" && event.target instanceof HTMLInputElement && event.target.type === "radio") {
    event.preventDefault();
    event.target.click();
  }
});

document.querySelector("#open-settings")!.addEventListener("click", () => settingsDialog.showModal());
accountTitle.addEventListener("click", () => accountDialog.showModal());
for (const button of document.querySelectorAll("[data-close-dialog]")) {
  button.addEventListener("click", () => button.closest("dialog")?.close());
}
document.querySelector("#minimize-app")!.addEventListener("click", () => {
  void Promise.resolve().then(() => getCurrentWindow().minimize()).catch(error => {
    message.textContent = t(String(error));
  });
});
document.addEventListener("visibilitychange", () => {
  document.documentElement.toggleAttribute("data-inactive", document.hidden || !document.hasFocus());
  if (!document.hidden) {
    renderAuth();
    void refresh();
  }
});
window.addEventListener("blur", () => document.documentElement.setAttribute("data-inactive", ""));
window.addEventListener("focus", () => document.documentElement.removeAttribute("data-inactive"));

quitApp.addEventListener("click", () => { void invoke("quit"); });

const appVersion = document.querySelector("#app-version")!;
const xrayVersion = document.querySelector("#xray-version")!;
let versions: { app: string; xray: string | null } | undefined;

function renderVersions() {
  if (!versions) return;
  appVersion.textContent = versions.app;
  xrayVersion.textContent = versions.xray ?? t("Недоступно");
}

void invoke<typeof versions>("app_versions").then(result => {
  versions = result;
  renderVersions();
}).catch(() => {
  appVersion.textContent = xrayVersion.textContent = "—";
});

void refresh();
window.setInterval(() => { void refresh(); }, 1500);

document.addEventListener("language-changed", () => {
  renderVersions();
  renderServers();
  renderControls();
  if (current.warning) message.textContent = t(current.warning);
});
