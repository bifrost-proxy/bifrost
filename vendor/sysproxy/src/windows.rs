use crate::{Error, Result, Sysproxy};
use std::{net::SocketAddr, str::FromStr};
use winapi::shared::ntdef::NULL;
use winapi::um::wininet::{
    InternetSetOptionA, INTERNET_OPTION_REFRESH, INTERNET_OPTION_SETTINGS_CHANGED,
};
use winreg::{enums, RegKey};

const SUB_KEY: &str = "SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Internet Settings";

impl Sysproxy {
    pub fn get_system_proxy() -> Result<Sysproxy> {
        let hkcu = RegKey::predef(enums::HKEY_CURRENT_USER);
        let cur_var = hkcu.open_subkey_with_flags(SUB_KEY, enums::KEY_READ)?;

        let enable = cur_var.get_value::<u32, _>("ProxyEnable")? == 1u32;
        let server = cur_var.get_value::<String, _>("ProxyServer")?;
        let server = server.as_str();

        let socket = SocketAddr::from_str(server).or(Err(Error::ParseStr))?;
        let host = socket.ip().to_string();
        let port = socket.port();

        let bypass = cur_var.get_value("ProxyOverride").unwrap_or("".into());

        Ok(Sysproxy {
            enable,
            host,
            port,
            bypass,
        })
    }

    pub fn set_system_proxy(&self) -> Result<()> {
        let hkcu = RegKey::predef(enums::HKEY_CURRENT_USER);
        let cur_var = hkcu.open_subkey_with_flags(SUB_KEY, enums::KEY_SET_VALUE)?;

        let enable = if self.enable { 1u32 } else { 0u32 };
        let server = format!("{}:{}", self.host, self.port);
        let bypass = self.bypass.as_str();

        cur_var.set_value("ProxyEnable", &enable)?;
        cur_var.set_value("ProxyServer", &server)?;
        cur_var.set_value("ProxyOverride", &bypass)?;

        // flush settings
        unsafe {
            InternetSetOptionA(NULL, INTERNET_OPTION_SETTINGS_CHANGED, NULL, 0);
            InternetSetOptionA(NULL, INTERNET_OPTION_REFRESH, NULL, 0);
        }

        Ok(())
    }
}
