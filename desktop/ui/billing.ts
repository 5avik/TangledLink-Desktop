import { invoke } from "@tauri-apps/api/core";
import { locale, t } from "./i18n";

type Plan = { id: string; amountKopecks: number; days: number };
type Order = { id: string; status: string; days?: number | null; pendingSync: boolean; amountKopecks: number | null; promoCode: string | null; canPay: boolean };
type Billing = { plans: Plan[]; order: Order | null; instructions: string | null };

const dialog = document.querySelector<HTMLDialogElement>("#billing-dialog")!;
const form = document.querySelector<HTMLFormElement>("#billing-form")!;
const plan = document.querySelector<HTMLSelectElement>("#billing-plan")!;
const email = document.querySelector<HTMLInputElement>("#billing-email")!;
const promo = document.querySelector<HTMLInputElement>("#billing-promo")!;
const lockedNote = document.querySelector<HTMLElement>("#billing-locked")!;
const create = document.querySelector<HTMLButtonElement>("#billing-create")!;
const resume = document.querySelector<HTMLButtonElement>("#billing-resume")!;
const refresh = document.querySelector<HTMLButtonElement>("#billing-refresh")!;
const close = document.querySelector<HTMLButtonElement>("#billing-close")!;
const status = document.querySelector("#billing-status")!;
const info = document.querySelector("#billing-order")!;
const instructions = document.querySelector("#billing-instructions")!;
let account: string | null = null;
let billing: Billing | null = null;
let order: Order | null = null;
let busy = false;
let error = "";
let awaitingBrowser = false;
let synced: string | null = null;
let createdHere = false;

function money(kopecks: number) {
  return new Intl.NumberFormat(locale(), { style: "currency", currency: "RUB" }).format(kopecks / 100);
}

const durations: Record<string, string> = {
  month30: "1 месяц",
  month90: "3 месяца",
  month180: "6 месяцев",
  month360: "12 месяцев",
};

function duration(days: number) {
  const label = durations["month" + days];
  return label ? t(label) : days + " " + t("дн.");
}

function canCreate() {
  return !order || (order.status === "paid" && !order.pendingSync && !createdHere)
    || (order.status === "pending" && !order.canPay);
}

function render() {
  const selection = plan.value;
  plan.replaceChildren(...(billing?.plans ?? []).map(item => {
    const option = document.createElement("option");
    option.value = item.id;
    option.textContent = (durations[item.id] ? t(durations[item.id]) : duration(item.days)) + " · " + money(item.amountKopecks);
    return option;
  }));
  if (billing?.plans.some(item => item.id === selection)) plan.value = selection;
  else if (order?.days != null) {
    const existing = billing?.plans.find(item => item.days === order?.days);
    if (existing) plan.value = existing.id;
  }
  form.hidden = !billing?.plans.length;
  const editable = canCreate();
  lockedNote.hidden = editable;
  create.hidden = !editable;
  email.disabled = promo.disabled = !editable || busy;
  plan.disabled = busy;
  if (!editable) {
    email.value = "";
    promo.value = order?.promoCode ?? "";
  }
  resume.hidden = !order?.canPay;
  resume.disabled = refresh.disabled = close.disabled = busy;
  const state = order?.status === "paid"
    ? t(order.pendingSync ? "Оплата подтверждена. Подписка обновляется…" : "Оплата подтверждена.")
    : order?.status === "applying" || order?.status === "retry" ? t("Оплата обрабатывается.")
    : order?.status === "pending" ? t("Ожидаем оплату.")
    : order ? t("Проверь статус заказа.") : "";
  const total = order?.amountKopecks != null ? money(order.amountKopecks) : "";
  info.textContent = [state, order?.days != null ? duration(order.days) : "", total, order?.promoCode].filter(Boolean).join(" · ");
  instructions.textContent = order?.status === "pending" && !order.canPay ? billing?.instructions ?? "" : "";
  status.textContent = busy ? t("Выполняется…") : t(error);
}

async function syncPaid() {
  if (order?.status !== "paid" || order.pendingSync || synced === order.id) return;
  const id = order.id;
  await invoke("refresh_account", { billingSync: true });
  synced = id;
}

async function run(work: (owner: string) => Promise<void>) {
  if (busy || !account) return;
  const owner = account;
  busy = true;
  error = "";
  render();
  try {
    await work(owner);
    if (account === owner) await syncPaid();
  } catch (reason) {
    if (account === owner) error = String(reason);
  } finally {
    busy = false;
    render();
  }
}

function load(poll = false) {
  return run(async owner => {
    for (let attempt = 0; attempt < (poll ? 7 : 1); attempt++) {
      const result = await invoke<Billing>("billing_status");
      if (owner !== account || !dialog.open) return;
      billing = result;
      order = result.order;
      render();
      if (attempt === 6 || !order || !order.pendingSync && !["pending", "applying", "retry"].includes(order.status)) break;
      if (poll) await new Promise(resolve => window.setTimeout(resolve, 1500));
    }
  });
}

function checkout(action: { kind: "create"; planId: string; email: string; promoCode: string } | { kind: "resume"; orderId: string }) {
  return run(async owner => {
    const result = await invoke<Order>("billing_checkout", { action });
    if (owner !== account) return;
    order = result;
    createdHere = true;
    awaitingBrowser = result.canPay;
  });
}

function openBilling() {
  if (!account || dialog.open) return;
  billing = order = null;
  createdHere = awaitingBrowser = false;
  email.value = promo.value = "";
  dialog.showModal();
  void load();
}

export function setBillingAccount(next: string | null) {
  if (next === account) return;
  account = next;
  billing = order = null;
  synced = null;
  awaitingBrowser = false;
  if (dialog.open) dialog.close();
}

for (const button of document.querySelectorAll("[data-open-billing]")) button.addEventListener("click", openBilling);
form.addEventListener("submit", event => {
  event.preventDefault();
  if (!canCreate()) return;
  void checkout({kind: "create", planId: plan.value, email: email.value, promoCode: promo.value});
});
resume.addEventListener("click", () => {
  if (order) void checkout({kind: "resume", orderId: order.id});
});
refresh.addEventListener("click", () => { awaitingBrowser = false; void load(); });
close.addEventListener("click", () => dialog.close());
dialog.addEventListener("cancel", event => { if (busy) event.preventDefault(); });
window.addEventListener("focus", () => {
  if (dialog.open && awaitingBrowser && !busy) {
    awaitingBrowser = false;
    void load(true);
  }
});
document.addEventListener("language-changed", render);
