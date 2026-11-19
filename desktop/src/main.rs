#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod account;
mod latency;
mod settings;

use settings::Settings;
use std::{sync::Mutex, time::Duration};
use tangledlink_desktop::{Profile, subscription_profiles};
use tauri::{
    AppHandle, Manager, RunEvent, State, WebviewUrl, WebviewWindowBuilder, WindowEvent,
    image::Image,
    menu::{MenuBuilder, MenuItemBuilder},
    tray::TrayIconBuilder,
};

#[cfg(windows)]
use tangledlink_desktop::engine::Tunnel;
#[cfg(target_os = "linux")]
use tangledlink_desktop::linux::Tunnel;

#[derive(Default)]
struct Client {
    profiles: Vec<Profile>,
    account: Option<Account>,
    saved: Option<Settings>,
    manual_persisted: bool,
    warning: Option<String>,
    #[cfg(any(windows, target_os = "linux"))]
    tunnel: Option<Tunnel>,
    active: Option<usize>,
}

struct Account {
    id: String,
    session: account::Session,
    persisted: bool,
    profiles: Vec<Profile>,
    selected: usize,
    notice: Option<String>,
    usage: Option<account::Usage>,
    subscribed: Option<bool>,
}

type Shared = Mutex<Client>;

#[derive(serde::Serialize)]
struct Server {
    name: String,
    transport: String,
}

#[derive(serde::Serialize)]
struct Snapshot {
    servers: Vec<Server>,
    can_quit: bool,
    account: Option<String>,
    account_notice: Option<String>,
    subscribed: Option<bool>,
    usage: Option<account::Usage>,
    selected: usize,
    warning: Option<String>,
    active: Option<usize>,
}

impl Client {
    fn profiles(&self) -> &[Profile] {
        self.account
            .as_ref()
            .map_or(&self.profiles, |account| &account.profiles)
    }

    fn snapshot(&mut self) -> Result<Snapshot, String> {
        #[cfg(any(windows, target_os = "linux"))]
        if let Some(tunnel) = &mut self.tunnel
            && !tunnel.running()?
        {
            self.tunnel = None;
            self.active = None;
        }
        Ok(Snapshot {
            can_quit: cfg!(target_os = "linux"),
            servers: self
                .profiles()
                .iter()
                .map(|p| Server {
                    name: p.name.clone(),
                    transport: p.transport.clone(),
                })
                .collect(),
            active: self.active,
            selected: self.account.as_ref().map_or_else(
                || self.saved.as_ref().map_or(0, |saved| saved.selected),
                |account| account.selected,
            ),
            account: self.account.as_ref().map(|account| account.id.clone()),
            subscribed: self.account.as_ref().and_then(|account| account.subscribed),
            usage: match &self.account {
                Some(account) => account.usage,
                None => self.saved.as_ref().and_then(|saved| saved.usage),
            },
            account_notice: self
                .account
                .as_ref()
                .and_then(|account| account.notice.clone()),
            warning: self.warning.clone(),
        })
    }

    fn disconnect(&mut self) -> Result<(), String> {
        self.active = None;
        #[cfg(any(windows, target_os = "linux"))]
        if let Some(mut tunnel) = self.tunnel.take() {
            tunnel.stop()?;
        }
        Ok(())
    }
}

fn lock(shared: &Shared) -> Result<std::sync::MutexGuard<'_, Client>, String> {
    shared
        .lock()
        .map_err(|_| "Внутренняя ошибка клиента".into())
}

#[tauri::command]
async fn load_subscription(app: AppHandle, link: String) -> Result<Snapshot, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let link = match link.trim() {
            "" => {
                let shared = app.state::<Shared>();
                lock(&shared)?
                    .saved
                    .as_ref()
                    .map(|saved| saved.link.clone())
                    .ok_or("Введите ссылку подписки HTTPS")?
            }
            link => link.to_owned(),
        };
        let uri = url::Url::parse(&link).map_err(|_| "Введите ссылку подписки HTTPS")?;
        if uri.scheme() != "https"
            || uri.host_str().is_none()
            || !uri.username().is_empty()
            || uri.password().is_some()
            || uri.fragment().is_some()
        {
            return Err("Введите ссылку подписки HTTPS".into());
        }
        let config = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(15)))
            .max_redirects(0)
            .build();
        let response = ureq::Agent::new_with_config(config)
            .get(uri.as_str())
            .call()
            .map_err(|_| "Не удалось загрузить подписку")?;
        if !response.status().is_success() {
            return Err("Сервер подписки вернул ошибку".into());
        }
        let usage = response
            .headers()
            .get("subscription-userinfo")
            .and_then(|value| value.to_str().ok())
            .and_then(account::Usage::from_header);
        let text = response
            .into_body()
            .into_with_config()
            .limit(512 * 1024)
            .read_to_string()
            .map_err(|_| "Не удалось прочитать подписку")?;
        let profiles = subscription_profiles(&text)
            .map_err(|_| "В подписке нет поддерживаемых серверов VLESS REALITY")?;
        let shared = app.state::<Shared>();
        let mut client = lock(&shared)?;
        if client.snapshot()?.active.is_some() || client.account.is_some() {
            return Err("Отключи VPN и выйди из аккаунта перед импортом ссылки".into());
        }
        let selected = client
            .saved
            .as_ref()
            .filter(|saved| saved.link == uri.as_str())
            .and_then(|saved| client.profiles.get(saved.selected))
            .and_then(|previous| {
                let mut matching = profiles.iter().enumerate().filter(|(_, profile)| {
                    profile.name == previous.name && profile.transport == previous.transport
                });
                if let Some((index, _)) = matching.next()
                    && matching.next().is_none()
                {
                    return Some(index);
                }
                profiles.iter().position(|profile| {
                    profile.name == previous.name && profile.outbound == previous.outbound
                })
            })
            .unwrap_or(0);
        let saved = Settings {
            link: uri.into(),
            body: text,
            selected,
            usage,
        };
        let warning = persist(&app, &saved).err();
        #[cfg(windows)]
        if let Some(error) = warning.as_ref() {
            return Err(error.clone());
        }
        client.manual_persisted |= warning.is_none();
        client.saved = Some(saved);
        client.profiles = profiles;
        client.warning = warning;
        client.snapshot()
    })
    .await
    .map_err(|_| "Ошибка загрузки подписки".to_owned())?
}

#[cfg(any(windows, target_os = "linux"))]
fn cache_path(app: &AppHandle) -> tauri::Result<std::path::PathBuf> {
    Ok(app.path().app_local_data_dir()?.join("subscription.bin"))
}

fn persist(app: &AppHandle, saved: &Settings) -> Result<(), String> {
    #[cfg(any(windows, target_os = "linux"))]
    {
        let path = cache_path(app).map_err(|_| "Папка настроек недоступна")?;
        settings::write(&path, saved).map_err(|_| "Не удалось сохранить подписку на этом ПК")?;
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    let _ = (app, saved);
    Ok(())
}

#[tauri::command]
fn select_server(app: AppHandle, index: usize) -> Result<Snapshot, String> {
    let shared = app.state::<Shared>();
    let mut client = lock(&shared)?;
    if index >= client.profiles().len() || client.snapshot()?.active.is_some() {
        return Err("Сервер недоступен для выбора".into());
    }
    remember_server(&app, &mut client, index)?;
    client.snapshot()
}

fn remember_server(app: &AppHandle, client: &mut Client, index: usize) -> Result<(), String> {
    if let Some(account) = client.account.as_mut() {
        account.selected = index;
        return Ok(());
    }
    let saved = client.saved.as_mut().ok_or("Сначала загрузи подписку")?;
    if saved.selected == index {
        return Ok(());
    }
    let previous = saved.selected;
    saved.selected = index;
    if let Err(error) = persist(app, saved) {
        #[cfg(windows)]
        {
            saved.selected = previous;
            return Err(error);
        }
        #[cfg(not(windows))]
        {
            let _ = previous;
            client.warning = Some(error);
        }
    } else {
        client.manual_persisted = true;
    }
    Ok(())
}

#[tauri::command]
fn forget_subscription(app: AppHandle) -> Result<Snapshot, String> {
    let shared = app.state::<Shared>();
    let mut client = lock(&shared)?;
    if client.snapshot()?.active.is_some() {
        return Err("Сначала отключи VPN".into());
    }
    #[cfg(any(windows, target_os = "linux"))]
    if client.manual_persisted {
        let path = cache_path(&app).map_err(|_| "Папка настроек недоступна")?;
        if let Err(error) = settings::remove(&path)
            && error.kind() != std::io::ErrorKind::NotFound
        {
            return Err("Не удалось удалить подписку".into());
        }
    }
    client.saved = None;
    client.manual_persisted = false;
    client.profiles.clear();
    client.warning = None;
    client.snapshot()
}

#[cfg(any(windows, target_os = "linux"))]
fn restore(app: &AppHandle) -> Option<(Settings, Vec<Profile>)> {
    let result = (|| {
        let path = cache_path(app).map_err(|_| ())?;
        let Some(mut saved) = settings::read::<Settings>(&path).map_err(|_| ())? else {
            return Ok(None);
        };
        let profiles = subscription_profiles(&saved.body).map_err(|_| ())?;
        if saved.selected >= profiles.len() {
            saved.selected = 0;
        }
        Ok::<_, ()>(Some((saved, profiles)))
    })();
    match result {
        Ok(restored) => restored,
        Err(()) => {
            if let Ok(mut client) = lock(&app.state::<Shared>()) {
                client.warning = Some("Сохранённая подписка недоступна. Загрузи её снова.".into());
            }
            None
        }
    }
}

#[tauri::command]
async fn status(state: State<'_, Shared>) -> Result<Snapshot, String> {
    state
        .try_lock()
        .map_err(|_| "Операция ещё выполняется")?
        .snapshot()
}

#[tauri::command]
async fn measure_latency(app: AppHandle) -> Result<Vec<Option<u64>>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let endpoints = {
            let shared = app.state::<Shared>();
            let mut client = lock(&shared)?;
            if client.snapshot()?.active.is_some() {
                return Err("Отключи VPN для прямой проверки TCP".into());
            }
            if client.profiles().len() > 128 {
                return Err("Проверка поддерживает список до 128 серверов".into());
            }
            client
                .profiles()
                .iter()
                .map(|profile| {
                    let node = &profile.outbound["settings"]["vnext"][0];
                    let host = node["address"].as_str()?.to_owned();
                    let port = u16::try_from(node["port"].as_u64()?).ok()?;
                    Some((host, port))
                })
                .collect::<Vec<_>>()
        };
        let mut times = vec![None; endpoints.len()];
        let workers = 6;
        std::thread::scope(|scope| {
            let chunk = times.len().div_ceil(workers).max(1);
            for (batch, results) in endpoints.chunks(chunk).zip(times.chunks_mut(chunk)) {
                scope.spawn(move || {
                    for (endpoint, time) in batch.iter().zip(results) {
                        *time = endpoint
                            .as_ref()
                            .and_then(|(host, port)| latency::measure(host, *port));
                    }
                });
            }
        });
        Ok(times)
    })
    .await
    .map_err(|_| "Не удалось проверить задержку".to_owned())?
}

#[tauri::command]
async fn connect(app: AppHandle, index: usize) -> Result<Snapshot, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let shared = app.state::<Shared>();
        let mut client = lock(&shared)?;
        if client.snapshot()?.active == Some(index) {
            return client.snapshot();
        }
        if client
            .account
            .as_ref()
            .is_some_and(|account| account.session.expired())
        {
            return Err("Сессия истекла. Выйди из аккаунта и войди снова.".into());
        }
        let profile = client.profiles().get(index).ok_or("Сервер не найден")?;
        #[cfg(not(any(windows, target_os = "linux")))]
        {
            let _ = profile;
            Err("В этой сборке туннель доступен на Windows".into())
        }
        #[cfg(any(windows, target_os = "linux"))]
        {
            let previous = client.active.and_then(|old| {
                client
                    .profiles()
                    .get(old)
                    .map(|profile| (old, profile.clone()))
            });
            let target = profile.clone();
            #[cfg(windows)]
            let start = {
                let core = std::env::current_exe()
                    .map_err(|_| "Не удалось определить папку приложения")?
                    .with_file_name("xray")
                    .join("xray.exe");
                if !core.is_file() {
                    return Err("Не найден Xray рядом с приложением".into());
                }
                move |profile: Profile| Tunnel::start(&core, profile.outbound)
            };
            #[cfg(target_os = "linux")]
            let start = {
                if !std::path::Path::new(tangledlink_desktop::linux::HELPER).is_file() {
                    return Err("Установи системный помощник TangledLink из пакета Linux.".into());
                }
                |profile: Profile| Tunnel::start(&profile.source)
            };
            client.disconnect()?;
            match start_or_restore((index, target), previous, start) {
                SwitchResult::Started {
                    index,
                    tunnel,
                    restored,
                } => {
                    client.tunnel = Some(tunnel);
                    client.active = Some(index);
                    if restored {
                        return Err(
                            "Новый туннель не запустился. Прежнее подключение восстановлено."
                                .into(),
                        );
                    }
                    if client.snapshot()?.selected != index
                        && remember_server(&app, &mut client, index).is_err()
                    {
                        client.warning =
                            Some("Сервер подключён. Не удалось сохранить выбор.".into());
                    }
                    client.snapshot()
                }
                SwitchResult::Failed {
                    attempted_restore: true,
                    ..
                } => Err(
                    "Не удалось запустить новый туннель или восстановить прежний. VPN отключён."
                        .into(),
                ),
                SwitchResult::Failed { error, .. } => Err(error.into()),
            }
        }
    })
    .await
    .map_err(|_| "Ошибка запуска VPN".to_owned())?
}

#[cfg(any(windows, target_os = "linux", test))]
enum SwitchResult<T, E> {
    Started {
        index: usize,
        tunnel: T,
        restored: bool,
    },
    Failed {
        error: E,
        attempted_restore: bool,
    },
}

#[cfg(any(windows, target_os = "linux", test))]
fn start_or_restore<P, T, E>(
    target: (usize, P),
    previous: Option<(usize, P)>,
    mut start: impl FnMut(P) -> Result<T, E>,
) -> SwitchResult<T, E> {
    let (index, profile) = target;
    match start(profile) {
        Ok(tunnel) => SwitchResult::Started {
            index,
            tunnel,
            restored: false,
        },
        Err(error) => {
            let attempted_restore = previous.is_some();
            if let Some((index, profile)) = previous
                && let Ok(tunnel) = start(profile)
            {
                return SwitchResult::Started {
                    index,
                    tunnel,
                    restored: true,
                };
            }
            SwitchResult::Failed {
                error,
                attempted_restore,
            }
        }
    }
}

#[tauri::command]
async fn disconnect(app: AppHandle) -> Result<Snapshot, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let shared = app.state::<Shared>();
        let mut client = lock(&shared)?;
        client.disconnect()?;
        client.snapshot()
    })
    .await
    .map_err(|_| "Ошибка остановки VPN".to_owned())?
}

#[derive(serde::Serialize)]
struct AuthResult {
    email_required: bool,
    snapshot: Option<Snapshot>,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct SavedAccount {
    id: String,
    session: account::Session,
}

#[cfg(any(windows, target_os = "linux"))]
fn session_path(app: &AppHandle) -> Result<std::path::PathBuf, String> {
    Ok(app
        .path()
        .app_local_data_dir()
        .map_err(|_| "Папка настроек недоступна")?
        .join("session.bin"))
}

fn forget_session(app: &AppHandle) -> Result<(), String> {
    #[cfg(any(windows, target_os = "linux"))]
    if let Err(error) = settings::remove(&session_path(app)?)
        && error.kind() != std::io::ErrorKind::NotFound
    {
        return Err("Не удалось удалить сохранённую сессию".into());
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    let _ = app;
    Ok(())
}

#[cfg(any(windows, target_os = "linux"))]
fn restore_session(app: &AppHandle, client: &mut Client) {
    let result = session_path(app).and_then(|path| {
        settings::read::<SavedAccount>(&path)
            .map_err(|_| "Не удалось открыть сохранённую сессию".into())
    });
    match result {
        Ok(Some(saved)) if !saved.session.expired() => {
            client.account = Some(Account {
                id: saved.id,
                session: saved.session,
                persisted: true,
                profiles: Vec::new(),
                selected: 0,
                notice: Some("Загружаем серверы аккаунта…".into()),
                usage: None,
                subscribed: None,
            });
        }
        Ok(Some(_)) => {
            client.warning = Some(
                forget_session(app)
                    .err()
                    .unwrap_or_else(|| "Сессия истекла. Войди снова.".into()),
            );
        }
        Ok(None) => {}
        Err(error) => client.warning = Some(error),
    }
}

#[derive(Default)]
struct AccountUpdate {
    profiles: Vec<Profile>,
    notice: Option<String>,
    usage: Option<account::Usage>,
    subscribed: Option<bool>,
}

fn account_profiles(agent: &ureq::Agent, session: &account::Session) -> Option<AccountUpdate> {
    if session.expired() {
        return None;
    }
    Some(match account::subscription(agent, session) {
        Ok(account::Subscription::Available { body, usage }) => {
            let result = subscription_profiles(&body);
            AccountUpdate {
                notice: result
                    .as_ref()
                    .err()
                    .map(|_| "В подписке аккаунта нет поддерживаемых серверов.".into()),
                profiles: result.unwrap_or_default(),
                usage,
                subscribed: Some(true),
            }
        }
        Ok(account::Subscription::Inactive) => AccountUpdate {
            notice: Some("У аккаунта нет активной подписки.".into()),
            subscribed: Some(false),
            ..Default::default()
        },
        Ok(account::Subscription::Expired) => return None,
        Err(error) => AccountUpdate {
            notice: Some(error),
            ..Default::default()
        },
    })
}

#[tauri::command]
async fn login_account(
    app: AppHandle,
    id: String,
    password: String,
    email_code: Option<String>,
) -> Result<AuthResult, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let id = id.trim().to_owned();
        if id.is_empty() || password.is_empty() {
            return Err("Введи логин и пароль".into());
        }
        let shared = app.state::<Shared>();
        let mut client = lock(&shared)?;
        if client.snapshot()?.active.is_some() || client.account.is_some() {
            return Err("Для входа отключи VPN и выйди из текущего аккаунта".into());
        }
        let agent = account::agent();
        let Some(session) = account::login(
            &agent,
            account::Login {
                id: &id,
                password: &password,
                email_code: email_code.as_deref(),
            },
        )?
        else {
            return Ok(AuthResult {
                email_required: true,
                snapshot: None,
            });
        };
        let update = account_profiles(&agent, &session).ok_or("Сессия истекла. Войди снова.")?;
        client.warning = None;
        #[cfg(any(windows, target_os = "linux"))]
        let persisted = session_path(&app)
            .and_then(|path| {
                settings::write(
                    &path,
                    &SavedAccount {
                        id: id.clone(),
                        session: session.clone(),
                    },
                )
                .map_err(|_| "Не удалось сохранить сессию".into())
            })
            .is_ok();
        #[cfg(not(any(windows, target_os = "linux")))]
        let persisted = false;
        if !persisted {
            client.warning = Some("Вход выполнен. Система не сохранила сессию.".into());
        }
        client.account = Some(Account {
            id,
            session,
            persisted,
            profiles: update.profiles,
            selected: 0,
            notice: update.notice,
            usage: update.usage,
            subscribed: update.subscribed,
        });
        Ok(AuthResult {
            email_required: false,
            snapshot: Some(client.snapshot()?),
        })
    })
    .await
    .map_err(|_| "Ошибка входа".to_owned())?
}

#[tauri::command]
async fn refresh_account(app: AppHandle) -> Result<Snapshot, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let shared = app.state::<Shared>();
        let session = {
            let mut client = lock(&shared)?;
            if client.snapshot()?.active.is_some() {
                return Err("Сначала отключи VPN".into());
            }
            client
                .account
                .as_ref()
                .ok_or("Сначала войди в аккаунт")?
                .session
                .clone()
        };
        let update = account_profiles(&account::agent(), &session);
        let mut client = lock(&shared)?;
        if client.active.is_some() {
            return client.snapshot();
        }
        let Some(account) = client
            .account
            .as_mut()
            .filter(|account| account.session == session)
        else {
            return client.snapshot();
        };
        let Some(update) = update else {
            client.disconnect()?;
            client.account = None;
            client.warning = Some(
                forget_session(&app)
                    .err()
                    .unwrap_or_else(|| "Сессия истекла. Войди снова.".into()),
            );
            return client.snapshot();
        };
        account.selected = account
            .profiles
            .get(account.selected)
            .and_then(|previous| {
                update.profiles.iter().position(|profile| {
                    profile.name == previous.name && profile.transport == previous.transport
                })
            })
            .unwrap_or(0);
        account.profiles = update.profiles;
        account.notice = update.notice;
        account.usage = update.usage;
        account.subscribed = update.subscribed;
        client.snapshot()
    })
    .await
    .map_err(|_| "Ошибка обновления аккаунта".to_owned())?
}

#[tauri::command]
async fn logout_account(app: AppHandle) -> Result<Snapshot, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let shared = app.state::<Shared>();
        let mut client = lock(&shared)?;
        client.disconnect()?;
        if client
            .account
            .as_ref()
            .is_some_and(|account| account.persisted)
        {
            forget_session(&app)?;
        }
        client.warning = None;
        if let Some(account) = client.account.take()
            && !account::logout(&account::agent(), &account.session)
        {
            client.warning =
                Some("Выход выполнен на этом ПК. Сервер не подтвердил завершение сессии.".into());
        }
        client.snapshot()
    })
    .await
    .map_err(|_| "Ошибка выхода".to_owned())?
}

#[tauri::command]
fn quit(app: AppHandle) {
    app.exit(0);
}

fn show_window(app: &AppHandle) -> tauri::Result<()> {
    if let Some(window) = app.get_webview_window("main") {
        window.show()?;
        return window.set_focus();
    }
    WebviewWindowBuilder::new(app, "main", WebviewUrl::App("index.html".into()))
        .title("TangledLink")
        .inner_size(690.0, 690.0)
        .min_inner_size(530.0, 510.0)
        .center()
        .build()?;
    Ok(())
}

fn main() {
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _, _| {
            let _ = show_window(app);
        }))
        .manage(Shared::default())
        .invoke_handler(tauri::generate_handler![
            load_subscription,
            login_account,
            refresh_account,
            logout_account,
            select_server,
            forget_subscription,
            status,
            connect,
            measure_latency,
            disconnect,
            quit
        ])
        .setup(|app| {
            #[cfg(any(windows, target_os = "linux"))]
            if let Some((saved, profiles)) = restore(app.handle()) {
                let shared = app.state::<Shared>();
                let mut client = lock(&shared)?;
                client.saved = Some(saved);
                client.manual_persisted = true;
                client.profiles = profiles;
            }
            #[cfg(any(windows, target_os = "linux"))]
            {
                let shared = app.state::<Shared>();
                let mut client = lock(&shared)?;
                restore_session(app.handle(), &mut client);
                if client.account.is_some() {
                    let handle = app.handle().clone();
                    tauri::async_runtime::spawn(async move {
                        let _ = refresh_account(handle).await;
                    });
                }
            }
            let open = MenuItemBuilder::with_id("open", "Открыть TangledLink").build(app)?;
            let exit = MenuItemBuilder::with_id("quit", "Отключить VPN и выйти").build(app)?;
            let menu = MenuBuilder::new(app).items(&[&open, &exit]).build()?;
            TrayIconBuilder::with_id("main")
                .icon(Image::from_bytes(include_bytes!("../icons/icon.png"))?)
                .tooltip("TangledLink")
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, event| match event.id().as_ref() {
                    "open" => {
                        let _ = show_window(app);
                    }
                    "quit" => app.exit(0),
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    if matches!(
                        event,
                        tauri::tray::TrayIconEvent::Click {
                            button: tauri::tray::MouseButton::Left,
                            button_state: tauri::tray::MouseButtonState::Up,
                            ..
                        }
                    ) {
                        let _ = show_window(tray.app_handle());
                    }
                })
                .build(app)?;
            show_window(app.handle())?;
            Ok(())
        })
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.destroy();
            }
        })
        .build(tauri::generate_context!())
        .expect("TangledLink could not start");
    app.run(|handle, event| match event {
        RunEvent::ExitRequested {
            code: None, api, ..
        } => api.prevent_exit(),
        RunEvent::Exit => {
            if let Ok(mut client) = lock(&handle.state::<Shared>()) {
                let _ = client.disconnect();
            }
        }
        _ => {}
    });
}

#[cfg(all(windows, test))]
mod session_tests {
    use super::*;

    #[test]
    fn switching_restores_previous_tunnel_only_on_failure() {
        let start = |candidate| {
            if candidate == "broken" {
                Err("startup failed")
            } else {
                Ok(candidate)
            }
        };
        assert!(matches!(
            start_or_restore((1, "ready"), Some((0, "previous")), start),
            SwitchResult::Started {
                index: 1,
                tunnel: "ready",
                restored: false
            }
        ));
        assert!(matches!(
            start_or_restore((1, "broken"), Some((0, "previous")), start),
            SwitchResult::Started {
                index: 0,
                tunnel: "previous",
                restored: true
            }
        ));
        assert!(matches!(
            start_or_restore((1, "broken"), Some((0, "broken")), start),
            SwitchResult::Failed {
                error: "startup failed",
                attempted_restore: true
            }
        ));
    }

    #[test]
    fn saved_account_preserves_session_without_exposing_token_on_disk() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("session.bin");
        let seed = "test-only-session-token";
        let saved = SavedAccount {
            id: "test-account".into(),
            session: serde_json::from_value(serde_json::json!({
                "token": seed, "expires_at": 4102444800u64
            }))
            .unwrap(),
        };
        settings::write(&path, &saved).unwrap();
        assert!(
            !std::fs::read(&path)
                .unwrap()
                .windows(seed.len())
                .any(|part| part == seed.as_bytes())
        );
        let restored: SavedAccount = settings::read(&path).unwrap().unwrap();
        assert_eq!(restored.id, saved.id);
        assert!(restored.session == saved.session);
        assert!(!restored.session.expired());
    }
}
