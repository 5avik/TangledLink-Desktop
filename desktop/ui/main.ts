import { invoke } from "@tauri-apps/api/core";

type Server = { name: string; transport: string };
type Usage = { used: number | null; limit: number | null; expires_at: number | null };
type Snapshot = { can_quit: boolean; servers: Server[]; active: number | null; selected: number; warning: string | null; account: string | null; account_notice: string | null; subscribed: boolean | null; usage: Usage | null };

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
const load = document.querySelector<HTMLButtonElement>("#load")!;
const link = document.querySelector<HTMLInputElement>("#link")!;
const form = document.querySelector<HTMLFormElement>("#subscription")!;
const forget = document.querySelector<HTMLButtonElement>("#forget")!;
const message = document.querySelector<HTMLParagraphElement>("#message")!;

const accountTitle = document.querySelector<HTMLElement>("#account-title")!;
const loginForm = document.querySelector<HTMLFormElement>("#login")!;
const accountId = document.querySelector<HTMLInputElement>("#account-id")!;
const password = document.querySelector<HTMLInputElement>("#account-password")!;
const code = document.querySelector<HTMLInputElement>("#account-code")!;
const challenge = document.querySelector<HTMLDivElement>("#challenge")!;
const signedIn = document.querySelector<HTMLDivElement>("#signed-in")!;
const accountNotice = document.querySelector<HTMLParagraphElement>("#account-notice")!;
const loginButton = document.querySelector<HTMLButtonElement>("#login-submit")!;
const refreshButton = document.querySelector<HTMLButtonElement>("#account-refresh")!;
const logoutButton = document.querySelector<HTMLButtonElement>("#account-logout")!;

let current: Snapshot = { can_quit: false, servers: [], active: null, selected: 0, warning: null, account: null, account_notice: null, subscribed: null, usage: null };

let latency: (number | null)[] = [];
let sortByLatency = false;
let pendingServer: number | null = null;
let busy = false;
let refreshing = false;

const stats = document.querySelector<HTMLElement>("#subscription-stats")!;
const subscriptionState = document.querySelector<HTMLParagraphElement>("#subscription-state")!;
const subscriptionUsage = document.querySelector<HTMLParagraphElement>("#subscription-usage")!;
const subscriptionExpiry = document.querySelector<HTMLParagraphElement>("#subscription-expiry")!;
const amount = new Intl.NumberFormat("ru", { maximumFractionDigits: 1 });
const dateFormat = new Intl.DateTimeFormat("ru", { dateStyle: "medium", timeStyle: "short" });

function formatBytes(bytes: number) {
  const units = ["Б", "КиБ", "МиБ", "ГиБ", "ТиБ", "ПиБ", "ЭиБ"];
  const index = bytes > 0 ? Math.min(Math.floor(Math.log(bytes) / Math.log(1024)), units.length - 1) : 0;
  return amount.format(bytes / 1024 ** index) + " " + units[index];
}

function renderSubscriptionStats() {
  const { subscribed, usage } = current;
  let traffic = "";
  if (usage?.used != null) {
    traffic = "Трафик " + formatBytes(usage.used);
    if (usage.limit === 0) traffic += " · без лимита";
    else if (usage.limit !== null) traffic += " из " + formatBytes(usage.limit);
  } else if (usage?.limit != null) {
    traffic = usage.limit === 0 ? "Трафик без лимита" : "Лимит трафика " + formatBytes(usage.limit);
  }
  const expiry = usage?.expires_at ? new Date(usage.expires_at * 1000) : null;
  subscriptionState.textContent = subscribed === null ? "" : subscribed ? "Подписка активна" : "Подписка неактивна";
  subscriptionUsage.textContent = traffic;
  subscriptionExpiry.textContent = expiry && Number.isFinite(expiry.getTime())
    ? "Действует до " + dateFormat.format(expiry) : "";
  for (const field of [subscriptionState, subscriptionUsage, subscriptionExpiry]) {
    field.hidden = !field.textContent;
  }
  stats.hidden = ![subscriptionState, subscriptionUsage, subscriptionExpiry].some(field => !field.hidden);
}

function renderControls() {
  renderSubscriptionStats();
  quitApp.hidden = !current.can_quit;
  quitApp.disabled = busy;
  const active = current.active !== null;
  status.textContent = active ? "Туннель запущен" : "VPN отключён";
  status.classList.toggle("online", active);
  activeName.textContent = current.servers[current.active ?? current.selected]?.name
    ?? "Выбери сервер для подключения";
  toggle.textContent = active ? "Отключить" : "Подключить";
  toggle.disabled = busy || current.servers.length === 0;
  switchServer.hidden = !active || pendingServer === null || pendingServer === current.active;
  switchServer.disabled = busy;
  toggle.classList.toggle("primary", switchServer.hidden);
  toggle.classList.toggle("secondary", !switchServer.hidden);
  const signed = current.account !== null;
  accountTitle.textContent = signed ? "Аккаунт · " + current.account : "Войти в аккаунт";
  loginForm.hidden = signed;
  signedIn.hidden = !signed;
  form.hidden = signed;
  loginButton.disabled = busy || active;
  refreshButton.disabled = busy || active;
  logoutButton.disabled = busy;
  accountNotice.textContent = current.account_notice ?? "";
  load.disabled = busy || active;
  link.disabled = busy || active;
  forget.disabled = busy || active;
  forget.hidden = current.servers.length === 0;
  link.required = current.servers.length === 0;
  load.textContent = current.servers.length ? "Обновить" : "Загрузить";
  link.placeholder = current.servers.length ? "Новая ссылка (необязательно)" : "https://…";
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

function renderServers() {
  list.replaceChildren();
  const query = search.value.trim().toLocaleLowerCase();
  const visible = current.servers
    .map((server, index) => ({ server, index }))
    .filter(({server}) => (server.name + " " + (server.transport === "raw" ? "tcp" : server.transport)).toLocaleLowerCase().includes(query));
  if (sortByLatency) {
    const time = (index: number) => latency[index] ?? Number.MAX_SAFE_INTEGER;
    visible.sort((a, b) => time(a.index) - time(b.index));
  }
  if (!visible.length) {
    const empty = document.createElement("p");
    empty.className = "empty";
    empty.textContent = current.servers.length ? "По запросу ничего не найдено" : "Загрузи подписку, чтобы увидеть серверы";
    list.append(empty);
    return;
  }
  visible.forEach(({server, index}) => {
    const row = document.createElement("label");
    row.className = "server";
    const radio = document.createElement("input");
    radio.type = "radio";
    radio.name = "server";
    radio.value = String(index);
    radio.checked = index === current.selected;
    radio.addEventListener("change", () => {
      if (current.active !== null) {
        pendingServer = index === current.active ? null : index;
        renderControls();
      } else {
        void act(() => invoke("select_server", { index }), () => {});
      }
    });
    const name = document.createElement("span");
    name.className = "server-name";
    name.textContent = server.name;
    const transport = document.createElement("span");
    transport.className = "transport";
    transport.textContent = server.transport === "raw" ? "TCP" : server.transport.toUpperCase();
    row.append(radio, name, transport);
    if (latency.length > 0) {
      const delay = document.createElement("span");
      delay.className = "transport delay";
      delay.textContent = latency[index] == null ? "Нет ответа" : latency[index] + " мс";
      row.append(delay);
    }
    list.append(row);
  });
}

async function act(operation: () => Promise<Snapshot>, result: (snapshot: Snapshot) => void, pendingMessage = "Выполняется…") {
  if (busy) return;
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
    message.textContent = String(error);
  } finally {
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
    message.textContent = "Проверены TCP-подключения к серверам";
  }, "Проверяем задержку TCP…");
});

form.addEventListener("submit", (event) => {
  event.preventDefault();
  void act(() => invoke("load_subscription", { link: link.value }), (snapshot) => {
    clearLatency();
    link.value = "";
    renderServers();
    message.textContent = "Загружено серверов — " + snapshot.servers.length;
  });
});

toggle.addEventListener("click", () => {
  const command = current.active === null ? "connect" : "disconnect";
  pendingServer = null;
  void act(() => invoke(command, { index: current.selected }), () => {});
});

switchServer.addEventListener("click", () => {
  if (pendingServer === null || current.active === null) return;
  const index = pendingServer;
  pendingServer = null;
  void act(() => invoke("connect", { index }), () => {
    message.textContent = "Сервер переключён";
  }, "Переключаем сервер…");
});

forget.addEventListener("click", () => {
  void act(() => invoke("forget_subscription"), () => {
    renderServers();
    message.textContent = "Подписка удалена с этого ПК";
  });
});

loginForm.addEventListener("submit", (event) => {
  event.preventDefault();
  let requiresCode = false;
  void act(async () => {
    const result = await invoke<{ email_required: boolean; snapshot: Snapshot | null }>("login_account", {
      id: accountId.value, password: password.value, emailCode: code.value || null,
    });
    requiresCode = result.email_required;
    return result.snapshot ?? current;
  }, () => {
    if (requiresCode) {
      challenge.hidden = false;
      message.textContent = "Введи код, отправленный на почту аккаунта.";
      return;
    }
    loginForm.reset();
    challenge.hidden = true;
    renderServers();
  });
});

refreshButton.addEventListener("click", () => {
  void act(() => invoke("refresh_account"), () => { clearLatency(); renderServers(); });
});

logoutButton.addEventListener("click", () => {
  void act(() => invoke("logout_account"), (snapshot) => {
    renderServers();
    message.textContent = snapshot.warning ?? "Ты вышел из аккаунта";
  });
});

async function refresh() {
  if (busy || refreshing) return;
  refreshing = true;
  try {
    const snapshot = await invoke<Snapshot>("status");
    if (busy) return;
    const changed = snapshot.account !== current.account || JSON.stringify(snapshot.servers) !== JSON.stringify(current.servers);
    if (current.active !== snapshot.active) pendingServer = null;
    if (current.active !== null && snapshot.active === null) {
      message.textContent = "Туннель остановился";
    }
    current = snapshot;
    if (changed) { clearLatency(); renderServers(); }
    if (snapshot.warning) message.textContent = snapshot.warning;
    renderControls();
  } catch (error) {
    if (!busy) message.textContent = String(error);
  } finally {
    refreshing = false;
  }
}

quitApp.addEventListener("click", () => { void invoke("quit"); });

void refresh();
window.setInterval(() => { void refresh(); }, 1500);
