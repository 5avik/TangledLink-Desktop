#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod account;
#[cfg(target_os = "linux")]
mod geofiles;
mod latency;
mod settings;
#[cfg(target_os = "linux")]
mod updater;

use std::sync::Mutex;
use tangledlink_desktop::{Profile, subscription_profiles};
use tauri::{
    AppHandle, Manager, RunEvent, State, WebviewUrl, WebviewWindowBuilder, WindowEvent,
    image::Image,
    menu::{MenuBuilder, MenuItem, MenuItemBuilder},
    tray::TrayIconBuilder,
};

#[cfg(windows)]
use tangledlink_desktop::engine::Tunnel;
#[cfg(target_os = "linux")]
use tangledlink_desktop::linux::Tunnel;

#[derive(Default)]
struct Client {
    #[cfg(target_os = "linux")]
    mtu: Option<u16>,
    #[cfg(target_os = "linux")]
    installing_update: bool,
    #[cfg(target_os = "linux")]
    geo_bundle: Option<std::path::PathBuf>,
    #[cfg(target_os = "linux")]
    routing: (bool, bool),
    #[cfg(target_os = "linux")]
    core_options: tangledlink_desktop::linux::Options,
    #[cfg(target_os = "linux")]
    active_linux_settings: Option<LinuxSettings>,
    account: Option<Account>,
    tray_warning: Option<String>,
    warning: Option<String>,
    #[cfg(any(windows, target_os = "linux"))]
    tunnel: Option<Tunnel>,
    active: Option<usize>,
}

#[cfg(target_os = "linux")]
#[derive(Clone)]
struct LinuxSettings {
    mtu: u16,
    routing: (bool, bool),
    options: tangledlink_desktop::linux::Options,
}

struct Account {
    id: String,
    session: account::Session,
    persisted: bool,
    restoring: bool,
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
    checking_session: bool,
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
            .map_or(&[], |account| account.profiles.as_slice())
    }

    fn snapshot(&mut self) -> Result<Snapshot, String> {
        if self
            .account
            .as_ref()
            .is_some_and(|account| account.session.expired())
        {
            self.account = None;
            self.tray_warning = None;
            let stop_error = self.disconnect().err();
            self.warning =
                Some(stop_error.unwrap_or_else(|| "Сессия истекла. Войди снова.".into()));
        }
        #[cfg(any(windows, target_os = "linux"))]
        if let Some(tunnel) = &mut self.tunnel
            && !tunnel.running()?
        {
            self.tunnel = None;
            self.active = None;
        }
        Ok(Snapshot {
            can_quit: cfg!(target_os = "linux"),
            checking_session: self
                .account
                .as_ref()
                .is_some_and(|account| account.restoring),
            servers: self
                .profiles()
                .iter()
                .map(|p| Server {
                    name: p.name.clone(),
                    transport: p.transport.clone(),
                })
                .collect(),
            active: self.active,
            selected: self.account.as_ref().map_or(0, |account| account.selected),
            account: self.account.as_ref().map(|account| account.id.clone()),
            subscribed: self.account.as_ref().and_then(|account| account.subscribed),
            usage: self.account.as_ref().and_then(|account| account.usage),
            account_notice: self
                .account
                .as_ref()
                .and_then(|account| account.notice.clone()),
            warning: self.tray_warning.clone().or_else(|| self.warning.clone()),
        })
    }

    fn disconnect(&mut self) -> Result<(), String> {
        self.active = None;
        #[cfg(target_os = "linux")]
        {
            self.active_linux_settings = None;
        }
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
fn select_server(app: AppHandle, index: usize) -> Result<Snapshot, String> {
    let shared = app.state::<Shared>();
    let mut client = lock(&shared)?;
    if client.snapshot()?.active.is_some() {
        return Err("Сервер недоступен для выбора".into());
    }
    let account = client.account.as_mut().ok_or("Сначала войди в аккаунт")?;
    if index >= account.profiles.len() {
        return Err("Сервер недоступен для выбора".into());
    }
    account.selected = index;
    client.snapshot()
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
        client.tray_warning = None;
        connect_client(&mut client, index)
    })
    .await
    .map_err(|_| "Ошибка запуска VPN".to_owned())?
}

fn connect_client(client: &mut Client, index: usize) -> Result<Snapshot, String> {
    #[cfg(target_os = "linux")]
    if client.installing_update {
        return Err("Идёт установка обновления.".into());
    }
    if client.snapshot()?.active == Some(index) {
        return client.snapshot();
    }
    let account = client.account.as_ref().ok_or("Сначала войди в аккаунт")?;
    if account.session.expired() {
        return Err("Сессия истекла. Войди снова.".into());
    }
    let profile = account.profiles.get(index).ok_or("Сервер не найден")?;
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        let _ = profile;
        Err("В этой сборке туннель доступен на Windows и Linux".into())
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
            let core = windows_core_path()?;
            if !core.is_file() {
                return Err("Не найден Xray рядом с приложением".into());
            }
            move |profile: Profile| Tunnel::start(&core, profile.outbound)
        };
        #[cfg(target_os = "linux")]
        let (target, previous, start, settings, previous_settings) = {
            let settings = LinuxSettings {
                mtu: client.mtu.unwrap_or(1500),
                routing: client.routing,
                options: client.core_options.clone(),
            };
            if !settings.options.proxy_only
                && !std::path::Path::new(tangledlink_desktop::linux::HELPER).is_file()
            {
                return Err("Установи системный помощник TangledLink из пакета Linux.".into());
            }
            let previous_settings = client.active_linux_settings.clone();
            let previous = previous
                .zip(previous_settings.clone())
                .map(|((index, profile), config)| (index, (profile, config)));
            let bundle = client.geo_bundle.clone();
            let start = move |(profile, config): (Profile, LinuxSettings)| {
                let (local, mux) = config.routing;
                Tunnel::start(
                    &profile.source,
                    config.mtu,
                    local,
                    mux,
                    &config.options,
                    bundle
                        .as_deref()
                        .filter(|path| !matches!(path.try_exists(), Ok(false))),
                )
            };
            (
                (target, settings.clone()),
                previous,
                start,
                settings,
                previous_settings,
            )
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
                #[cfg(target_os = "linux")]
                {
                    client.active_linux_settings = if restored {
                        previous_settings
                    } else {
                        Some(settings)
                    };
                }
                if restored {
                    return Err(
                        "Новый туннель не запустился. Прежнее подключение восстановлено.".into(),
                    );
                }
                if let Some(account) = client.account.as_mut() {
                    account.selected = index;
                }
                client.snapshot()
            }
            SwitchResult::Failed {
                attempted_restore: true,
                ..
            } => Err(
                "Не удалось запустить новый туннель или восстановить прежний. VPN отключён.".into(),
            ),
            SwitchResult::Failed { error, .. } => Err(error.into()),
        }
    }
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
        client.tray_warning = None;
        client.disconnect()?;
        client.snapshot()
    })
    .await
    .map_err(|_| "Ошибка остановки VPN".to_owned())?
}

#[derive(serde::Serialize)]
struct AuthResult {
    email_required: bool,
    retry_after_seconds: u64,
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
                restoring: true,
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
    registration_email: Option<String>,
) -> Result<AuthResult, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let id = id.trim().to_owned();
        if id.is_empty() || password.is_empty() {
            return Err("Введи логин и пароль".into());
        }
        if registration_email.is_some() {
            if !(3..=32).contains(&id.len())
                || !id.bytes().all(|c| {
                    c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, b'_' | b'-')
                })
            {
                return Err(
                    "Логин: 3–32 символа, строчные латинские буквы, цифры, _ или дефис".into(),
                );
            }
            validate_new_password(&password)?;
        }
        validate_email_code(email_code.as_deref())?;
        let shared = app.state::<Shared>();
        let mut client = lock(&shared)?;
        if client.snapshot()?.active.is_some() || client.account.is_some() {
            return Err("Для входа отключи VPN и выйди из текущего аккаунта".into());
        }
        let agent = account::agent();
        let session = match account::authenticate(
            &agent,
            account::Login {
                id: &id,
                password: &password,
                email: registration_email.as_deref().map(str::trim),
                email_code: email_code.as_deref(),
            },
        )? {
            account::Authentication::SignedIn(session) => session,
            account::Authentication::EmailRequired(retry_after_seconds) => {
                return Ok(AuthResult {
                    email_required: true,
                    retry_after_seconds,
                    snapshot: None,
                });
            }
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
            restoring: false,
            profiles: update.profiles,
            selected: 0,
            notice: update.notice,
            usage: update.usage,
            subscribed: update.subscribed,
        });
        Ok(AuthResult {
            email_required: false,
            retry_after_seconds: 0,
            snapshot: Some(client.snapshot()?),
        })
    })
    .await
    .map_err(|_| "Ошибка входа".to_owned())?
}

fn validate_new_password(password: &str) -> Result<(), String> {
    if (12..=128).contains(&password.encode_utf16().count()) {
        Ok(())
    } else {
        Err("Пароль должен содержать от 12 до 128 символов".into())
    }
}

fn validate_email_code(code: Option<&str>) -> Result<(), String> {
    if code.is_some_and(|code| code.len() != 6 || !code.bytes().all(|byte| byte.is_ascii_digit())) {
        return Err("Введи шесть цифр из письма".into());
    }
    Ok(())
}

#[tauri::command]
async fn recover_account(
    app: AppHandle,
    id: String,
    email: Option<String>,
    invite_code: Option<String>,
    email_code: Option<String>,
    password: Option<String>,
) -> Result<account::RecoveryResult, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let id = id.trim();
        if !(1..=128).contains(&id.len())
            || !id
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-'))
        {
            return Err("Проверь логин".into());
        }
        let shared = app.state::<Shared>();
        if lock(&shared)?.snapshot()?.account.is_some() {
            return Err("Сначала выйди из аккаунта".into());
        }
        validate_email_code(email_code.as_deref())?;
        if email_code.is_some() {
            validate_new_password(password.as_deref().unwrap_or_default())?;
        }
        account::recover(
            &account::agent(),
            account::Recovery {
                id,
                email: email.as_deref().map(str::trim),
                invite_code: invite_code.as_deref().map(str::trim),
                email_code: email_code.as_deref(),
                password: password.as_deref(),
            },
        )
    })
    .await
    .map_err(|_| "Ошибка восстановления пароля".to_owned())?
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
        if update.subscribed.is_none() {
            if account.restoring {
                client.account = None;
                client.warning =
                    Some("Не удалось подтвердить сохранённую сессию. Войди снова.".into());
            } else {
                account.notice = update.notice;
            }
            return client.snapshot();
        }
        account.restoring = false;
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

struct TrayItems([MenuItem<tauri::Wry>; 4]);

#[tauri::command]
fn set_language(tray: State<'_, TrayItems>, english: bool) -> Result<(), String> {
    let labels = if english {
        [
            "Open TangledLink",
            "Connect selected server",
            "Disconnect VPN",
            "Disconnect VPN and quit",
        ]
    } else {
        [
            "Открыть TangledLink",
            "Подключить выбранный сервер",
            "Отключить VPN",
            "Отключить VPN и выйти",
        ]
    };
    for (item, label) in tray.0.iter().zip(labels) {
        item.set_text(label).map_err(|error| error.to_string())?;
    }
    Ok(())
}

#[tauri::command]
async fn launch_split_app(app: AppHandle, program: String) -> Result<(), String> {
    #[cfg(target_os = "linux")]
    {
        return tauri::async_runtime::spawn_blocking(move || {
            let state = app.state::<Shared>();
            let mut client = lock(&state)?;
            if client.snapshot()?.active.is_none()
                || !client
                    .tunnel
                    .as_mut()
                    .ok_or("Сначала подключи VPN в режиме раздельного туннелирования.")?
                    .routes_apps()?
            {
                return Err("Сначала подключи VPN в режиме раздельного туннелирования.".into());
            }
            let mut process =
                tangledlink_desktop::linux::app_scope(std::path::Path::new(&program))?;
            drop(client);
            // systemd-run remains alive alongside the program. Catch immediate launch errors
            // and let a waiter reap it if the application continues running.
            std::thread::sleep(std::time::Duration::from_millis(250));
            match process
                .try_wait()
                .map_err(|_| "Не удалось запустить приложение через VPN.")?
            {
                Some(status) if !status.success() => {
                    Err("Не удалось запустить приложение через VPN.".into())
                }
                Some(_) => Ok(()),
                None => {
                    std::thread::spawn(move || {
                        let _ = process.wait();
                    });
                    Ok(())
                }
            }
        })
        .await
        .map_err(|_| "Не удалось запустить приложение через VPN.")?;
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (app, program);
        Err("Раздельное туннелирование доступно на Linux.".into())
    }
}

#[tauri::command]
fn geofiles_status(app: AppHandle) -> Result<serde_json::Value, String> {
    #[cfg(target_os = "linux")]
    return geofiles::status(&geofiles::bundle_path(&app)?);
    #[cfg(not(target_os = "linux"))]
    {
        let _ = app;
        Ok(serde_json::Value::Null)
    }
}

#[tauri::command]
async fn update_geofiles(app: AppHandle, source: usize) -> Result<serde_json::Value, String> {
    #[cfg(target_os = "linux")]
    {
        let path = geofiles::bundle_path(&app)?;
        return tauri::async_runtime::spawn_blocking(move || geofiles::update(&path, source))
            .await
            .map_err(|_| "Не удалось обновить геофайлы")?;
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (app, source);
        Ok(serde_json::Value::Null)
    }
}

#[tauri::command]
fn reset_geofiles(app: AppHandle) -> Result<serde_json::Value, String> {
    #[cfg(target_os = "linux")]
    return geofiles::reset(&geofiles::bundle_path(&app)?);
    #[cfg(not(target_os = "linux"))]
    {
        let _ = app;
        Ok(serde_json::Value::Null)
    }
}

#[tauri::command]
fn set_core_options(state: State<'_, Shared>, options: serde_json::Value) -> Result<(), String> {
    #[cfg(target_os = "linux")]
    {
        let options: tangledlink_desktop::linux::Options =
            serde_json::from_value(options).map_err(|_| "Некорректные параметры ядра")?;
        if !options.valid() {
            return Err("Проверь домены, IP, группы геофайлов и число потоков".into());
        }
        lock(&state)?.core_options = options;
    }
    #[cfg(not(target_os = "linux"))]
    let _ = (state, options);
    Ok(())
}

#[tauri::command]
fn set_linux_routing(state: State<'_, Shared>, local: bool, mux: bool) -> Result<(), String> {
    #[cfg(target_os = "linux")]
    {
        lock(&state)?.routing = (local, mux);
    }
    #[cfg(not(target_os = "linux"))]
    let _ = (state, local, mux);
    Ok(())
}

#[tauri::command]
fn set_vpn_mtu(state: State<'_, Shared>, mtu: u16) -> Result<(), String> {
    if !(1280..=1500).contains(&mtu) {
        return Err("MTU должен быть от 1280 до 1500".into());
    }
    #[cfg(target_os = "linux")]
    {
        lock(&state)?.mtu = Some(mtu);
    }
    #[cfg(not(target_os = "linux"))]
    let _ = state;
    Ok(())
}

#[tauri::command]
async fn change_account_password(
    app: AppHandle,
    current: String,
    new_password: String,
) -> Result<(), String> {
    validate_new_password(&new_password)?;
    tauri::async_runtime::spawn_blocking(move || {
        let shared = app.state::<Shared>();
        let session = lock(&shared)?
            .account
            .as_ref()
            .filter(|account| !account.restoring && !account.session.expired())
            .ok_or("Войди в аккаунт")?
            .session
            .clone();
        account::change_password(&account::agent(), &session, &current, &new_password)
    })
    .await
    .map_err(|_| "Не удалось изменить пароль".to_owned())?
}

#[tauri::command]
fn set_close_to_tray(state: State<'_, std::sync::atomic::AtomicBool>, enabled: bool) {
    state.store(enabled, std::sync::atomic::Ordering::Relaxed);
}

#[tauri::command]
fn quit(app: AppHandle) {
    app.exit(0);
}

fn tray_connection(app: &AppHandle, enable: bool) {
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let error = {
            let shared = app.state::<Shared>();
            // Duplicate clicks while a native authorization prompt is open must not queue.
            let Ok(mut client) = shared.try_lock() else {
                return;
            };
            let result = client.snapshot().and_then(|snapshot| {
                if enable {
                    if snapshot.active.is_some() {
                        return Ok(());
                    }
                    if snapshot.account.is_none() || snapshot.checking_session {
                        return Err("Сначала войди в аккаунт.".into());
                    }
                    if snapshot.servers.is_empty() {
                        return Err("В аккаунте нет доступных серверов.".into());
                    }
                    connect_client(&mut client, snapshot.selected).map(|_| ())
                } else {
                    client.disconnect()
                }
            });
            client.tray_warning = result.err();
            client.tray_warning.is_some()
        };
        if error {
            let window_app = app.clone();
            let _ = app.run_on_main_thread(move || {
                let _ = show_window(&window_app);
            });
        }
    });
}

fn show_window(app: &AppHandle) -> tauri::Result<()> {
    if let Some(window) = app.get_webview_window("main") {
        window.show()?;
        if window.is_minimized().unwrap_or(false) {
            let _ = window.unminimize();
        }
        return window.set_focus();
    }
    WebviewWindowBuilder::new(app, "main", WebviewUrl::App("index.html".into()))
        .title("TangledLink")
        .inner_size(400.0, 600.0)
        .min_inner_size(360.0, 510.0)
        .center()
        .build()?;
    Ok(())
}

#[cfg(windows)]
fn windows_core_path() -> Result<std::path::PathBuf, &'static str> {
    Ok(std::env::current_exe()
        .map_err(|_| "Не удалось определить папку приложения")?
        .with_file_name("xray")
        .join("xray.exe"))
}

#[derive(serde::Serialize)]
struct AppVersions {
    app: String,
    xray: Option<String>,
}

#[tauri::command]
async fn app_versions(app: AppHandle) -> AppVersions {
    let version = app.package_info().version.to_string();
    let xray = tauri::async_runtime::spawn_blocking(xray_version)
        .await
        .ok()
        .flatten();
    AppVersions { app: version, xray }
}

fn xray_version() -> Option<String> {
    #[cfg(not(any(windows, target_os = "linux")))]
    return None;
    #[cfg(any(windows, target_os = "linux"))]
    {
        use std::process::{Command, Stdio};
        #[cfg(windows)]
        let path = windows_core_path().ok()?;
        #[cfg(target_os = "linux")]
        let path = tangledlink_desktop::linux::CORE;
        let mut command = Command::new(path);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x08000000); // CREATE_NO_WINDOW
        }
        let result = command
            .arg("version")
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .ok()?;
        if !result.status.success() {
            return None;
        }
        let stdout = String::from_utf8_lossy(&result.stdout);
        let mut header = stdout.lines().next()?.split_whitespace();
        if header.next()? != "Xray" {
            return None;
        }
        Some(header.next()?.to_owned())
    }
}

#[tauri::command]
async fn check_for_update() -> Result<serde_json::Value, String> {
    #[cfg(target_os = "linux")]
    return tauri::async_runtime::spawn_blocking(|| {
        let update = updater::check()?;
        serde_json::to_value(update).map_err(|_| "Не удалось проверить обновления.".to_owned())
    })
    .await
    .map_err(|_| "Не удалось проверить обновления.".to_owned())?;
    #[cfg(not(target_os = "linux"))]
    Ok(serde_json::Value::Null)
}

#[cfg(target_os = "linux")]
#[derive(Default)]
struct UpdateLock(Mutex<()>);

#[tauri::command]
async fn install_update(app: AppHandle, version: String) -> Result<(), String> {
    #[cfg(target_os = "linux")]
    {
        use tauri::Emitter;
        return tauri::async_runtime::spawn_blocking(move || {
            let update_lock = app.state::<UpdateLock>();
            let _guard = update_lock
                .0
                .try_lock()
                .map_err(|_| "Идёт установка обновления.")?;
            let file = updater::download(&version, |received, total| {
                let _ = app.emit(
                    "update-progress",
                    serde_json::json!({"received": received, "total": total}),
                );
            })?;
            let state = app.state::<Shared>();
            let _ = app.emit("update-installing", ());
            if let Err(error) = updater::install(file, &version, || {
                let mut client = lock(&state)?;
                client.disconnect()?;
                client.installing_update = true;
                Ok(())
            }) {
                lock(&state)?.installing_update = false;
                return Err(error);
            }
            app.request_restart();
            Ok(())
        })
        .await
        .map_err(|_| "Не удалось установить обновление.")?;
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (app, version);
        Err("Подходящее обновление недоступно.".into())
    }
}

fn main() {
    #[cfg(target_os = "linux")]
    if unsafe { libc::geteuid() } == 0 {
        let mut args = std::env::args().skip(1);
        let result = match (args.next().as_deref(), args.next(), args.next()) {
            (Some("--install-update"), Some(version), None) => {
                updater::install_privileged(&version)
            }
            _ => Err("Запускай TangledLink от обычного пользователя."),
        };
        if let Err(error) = result {
            eprintln!("{error}");
            std::process::exit(1);
        }
        return;
    }
    let builder = tauri::Builder::default();
    #[cfg(target_os = "linux")]
    let builder = builder.manage(UpdateLock::default());
    let app = builder
        .plugin(tauri_plugin_single_instance::init(|app, _, _| {
            let _ = show_window(app);
        }))
        .manage(Shared::default())
        .manage(std::sync::atomic::AtomicBool::new(true))
        .invoke_handler(tauri::generate_handler![
            login_account,
            recover_account,
            refresh_account,
            logout_account,
            select_server,
            status,
            connect,
            measure_latency,
            disconnect,
            quit,
            set_close_to_tray,
            set_vpn_mtu,
            set_linux_routing,
            set_core_options,
            set_language,
            change_account_password,
            geofiles_status,
            update_geofiles,
            reset_geofiles,
            launch_split_app,
            app_versions,
            check_for_update,
            install_update
        ])
        .setup(|app| {
            #[cfg(any(windows, target_os = "linux"))]
            {
                let shared = app.state::<Shared>();
                let mut client = lock(&shared)?;
                #[cfg(target_os = "linux")]
                {
                    client.geo_bundle = Some(geofiles::bundle_path(app.handle())?);
                }
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
            let connect = MenuItemBuilder::with_id("connect-vpn", "Подключить выбранный сервер")
                .build(app)?;
            let disconnect =
                MenuItemBuilder::with_id("disconnect-vpn", "Отключить VPN").build(app)?;
            let menu = MenuBuilder::new(app)
                .items(&[&open, &connect, &disconnect, &exit])
                .build()?;
            app.manage(TrayItems([open, connect, disconnect, exit]));
            TrayIconBuilder::with_id("main")
                .icon(Image::from_bytes(include_bytes!("../icons/icon.png"))?)
                .tooltip("TangledLink")
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, event| match event.id().as_ref() {
                    "open" => {
                        let _ = show_window(app);
                    }
                    "connect-vpn" => tray_connection(app, true),
                    "disconnect-vpn" => tray_connection(app, false),
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
                if window
                    .state::<std::sync::atomic::AtomicBool>()
                    .load(std::sync::atomic::Ordering::Relaxed)
                {
                    let _ = window.destroy();
                } else {
                    window.app_handle().exit(0);
                }
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

#[cfg(all(target_os = "linux", test))]
mod linux_switch_tests {
    use super::*;

    #[test]
    fn failed_switch_restores_previous_connection_settings() {
        let previous = LinuxSettings {
            mtu: 1400,
            routing: (true, false),
            options: tangledlink_desktop::linux::Options {
                proxy_only: true,
                ..Default::default()
            },
        };
        let next = LinuxSettings {
            mtu: 1500,
            routing: (false, true),
            options: tangledlink_desktop::linux::Options {
                ipv6: true,
                ..Default::default()
            },
        };
        let result = start_or_restore((1, next), Some((0, previous)), |config| {
            if config.options.ipv6 {
                Err("configuration failed")
            } else {
                Ok(config)
            }
        });
        let SwitchResult::Started {
            index: 0,
            tunnel,
            restored: true,
        } = result
        else {
            panic!("previous tunnel was not restored");
        };
        assert_eq!(tunnel.mtu, 1400);
        assert_eq!(tunnel.routing, (true, false));
        assert!(tunnel.options.proxy_only);
    }
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
