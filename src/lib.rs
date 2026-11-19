use percent_encoding::percent_decode_str;
use serde_json::{Value, json};
use std::collections::HashMap;
use url::Url;

/// Builds an outbound for the supported VLESS + REALITY transports.
pub fn reality_outbound(profile: &str) -> Result<Value, &'static str> {
    let uri = Url::parse(profile.trim()).map_err(|_| "Invalid profile URL")?;
    if uri.scheme() != "vless" || uri.password().is_some() || !uri.path().is_empty() {
        return Err("Expected a VLESS profile URL");
    }

    let host = uri.host().ok_or("Missing server address")?;
    let address = match host {
        url::Host::Ipv6(ip) => ip.to_string(),
        _ => host.to_string(),
    };
    let port = uri
        .port()
        .filter(|port| *port > 0)
        .ok_or("Missing server port")?;
    let id = percent_decode_str(uri.username())
        .decode_utf8()
        .map_err(|_| "Invalid VLESS user ID")?;
    if id.is_empty() {
        return Err("Missing VLESS user ID");
    }

    let mut fields = HashMap::new();
    for (key, value) in uri.query_pairs() {
        match key.as_ref() {
            "type"
            | "security"
            | "encryption"
            | "flow"
            | "headerType"
            | "sni"
            | "fp"
            | "pbk"
            | "sid"
            | "spx"
            | "pqv"
            | "authority"
            | "serviceName"
            | "host"
            | "path"
            | "mode"
            | "extra"
            | "x_padding_bytes"
            | "support-x25519mlkem768" => {}
            _ => return Err("Profile contains an option unsupported by this prototype"),
        }
        if fields.insert(key, value).is_some() {
            return Err("Duplicate profile option");
        }
    }

    let field = |key| fields.get(key).map(|value| value.as_ref());
    if field("security") != Some("reality")
        || !matches!(field("headerType"), None | Some("" | "none"))
    {
        return Err("This preview supports VLESS with REALITY");
    }

    let public_key = field("pbk")
        .filter(|value| !value.is_empty())
        .ok_or("Missing REALITY public key")?;
    let server_name = field("sni")
        .filter(|value| !value.is_empty())
        .ok_or("Missing REALITY server name")?;
    let fingerprint = field("fp")
        .filter(|value| !value.is_empty())
        .unwrap_or("chrome");
    let mut reality = json!({
        "serverName": server_name,
        "fingerprint": fingerprint,
        "password": public_key,
        "shortId": field("sid").unwrap_or_default(),
        "spiderX": field("spx").unwrap_or_default()
    });
    if let Some(verify) = field("pqv") {
        reality["mldsa65Verify"] = json!(verify);
    }

    let mut stream = json!({"security": "reality", "realitySettings": reality});
    match field("type").unwrap_or("tcp") {
        "tcp" | "raw" => stream["network"] = json!("raw"),
        "grpc" => {
            stream["network"] = json!("grpc");
            stream["grpcSettings"] = json!({
                "authority": field("authority").unwrap_or_default(),
                "serviceName": field("serviceName").unwrap_or_default(),
                "multiMode": field("mode") == Some("multi"),
                "idle_timeout": 60,
                "health_check_timeout": 20
            });
        }
        "xhttp" => {
            let mut extra = match field("extra").filter(|value| !value.is_empty()) {
                Some(value) => serde_json::from_str::<Value>(value)
                    .map_err(|_| "Invalid XHTTP extra")?
                    .as_object()
                    .cloned()
                    .ok_or("Invalid XHTTP extra")?,
                None => serde_json::Map::new(),
            };
            if extra
                .keys()
                .any(|key| key != "mode" && key != "xPaddingBytes")
            {
                return Err("Unsupported XHTTP extra option");
            }
            if let Some(padding) = field("x_padding_bytes").filter(|value| !value.is_empty()) {
                extra
                    .entry("xPaddingBytes")
                    .or_insert_with(|| json!(padding));
            }
            stream["network"] = json!("xhttp");
            stream["xhttpSettings"] = json!({
                "host": field("host").unwrap_or_default(),
                "path": field("path").unwrap_or("/"),
                "mode": field("mode").unwrap_or("auto"),
                "extra": extra
            });
        }
        _ => return Err("Unsupported transport"),
    }

    Ok(json!({
        "protocol": "vless",
        "tag": "proxy",
        "settings": {
            "vnext": [{
                "address": address,
                "port": port,
                "users": [{
                    "id": id,
                    "encryption": field("encryption").unwrap_or("none"),
                    "flow": field("flow").unwrap_or_default()
                }]
            }]
        },
        "streamSettings": stream
    }))
}

pub fn windows_tun_config(outbound: Value) -> Value {
    json!({
        "log": { "loglevel": "none" },
        "inbounds": [{
            "protocol": "tun",
            "tag": "tun",
            "settings": {
                "name": "TangledLink",
                "desc": "TangledLink",
                "mtu": 1500,
                "gateway": ["172.30.254.1/30", "fd79:746e:676c::1/64"],
                "dns": ["1.1.1.1", "8.8.8.8"],
                "autoSystemRoutingTable": ["0.0.0.0/0", "::/0"],
                "autoOutboundsInterface": "auto",
                "autoSystemWfpBlockLeak": ["dns", "misconfigtun"]
            }
        }],
        "dns": { "servers": ["https://1.1.1.1/dns-query", "https://8.8.8.8/dns-query"] },
        "routing": { "rules": [
            { "inboundTag": ["tun"], "port": 53, "outboundTag": "dns-out" }
        ] },
        "outbounds": [outbound, { "protocol": "dns", "tag": "dns-out" }]
    })
}

#[derive(Clone)]
pub struct Profile {
    #[cfg(target_os = "linux")]
    pub source: String,
    pub name: String,
    pub transport: String,
    pub outbound: Value,
}

pub fn subscription_profiles(body: &str) -> Result<Vec<Profile>, &'static str> {
    use base64::{
        Engine,
        engine::general_purpose::{STANDARD_NO_PAD, URL_SAFE_NO_PAD},
    };
    let body = body.trim_start_matches('\u{feff}').trim();
    let decoded;
    let lines = if body
        .lines()
        .any(|line| line.trim_start().starts_with("vless://"))
    {
        body
    } else {
        let compact: String = body.chars().filter(|c| !c.is_ascii_whitespace()).collect();
        let unpadded = compact.trim_end_matches('=');
        let bytes = STANDARD_NO_PAD
            .decode(unpadded)
            .or_else(|_| URL_SAFE_NO_PAD.decode(unpadded))
            .map_err(|_| "Invalid subscription format")?;
        decoded = String::from_utf8(bytes).map_err(|_| "Invalid subscription text")?;
        &decoded
    };

    let profiles: Vec<_> = lines
        .lines()
        .filter_map(|line| {
            let outbound = reality_outbound(line).ok()?;
            let uri = Url::parse(line.trim()).ok()?;
            let name = uri
                .fragment()
                .map(|f| percent_decode_str(f).decode_utf8_lossy().into_owned())
                .filter(|name| !name.trim().is_empty())
                .unwrap_or_else(|| "Сервер".into());
            let transport = outbound["streamSettings"]["network"].as_str()?.to_owned();
            Some(Profile {
                #[cfg(target_os = "linux")]
                source: line.trim().to_owned(),
                name,
                transport,
                outbound,
            })
        })
        .collect();
    if profiles.is_empty() {
        return Err("No supported profiles in subscription");
    }
    Ok(profiles)
}

#[cfg(windows)]
pub mod engine;

#[cfg(target_os = "linux")]
pub mod linux;
