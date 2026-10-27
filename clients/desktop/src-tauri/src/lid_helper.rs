//! Setting up the power helper (macOS): the root daemon this app bundles
//! that keeps a closed Mac awake for the Host. macOS asks the user to allow
//! it in System Settings → Login Items the first time.

/// `SMAppServiceStatus` as the web view sees it.
pub fn status_name(code: isize) -> &'static str {
    match code {
        0 => "not_registered",
        1 => "enabled",
        2 => "requires_approval",
        3 => "not_found",
        _ => "unknown",
    }
}

/// Register the helper unless it already runs; when macOS wants the user's
/// approval, open Login Items for them.
#[tauri::command]
pub async fn lid_helper_setup() -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(imp::setup)
        .await
        .map_err(|e| e.to_string())?
}

#[cfg(target_os = "macos")]
mod imp {
    use objc2::msg_send;
    use objc2::rc::Retained;
    use objc2::runtime::{AnyClass, AnyObject};
    use objc2_foundation::{NSError, NSString};

    use super::status_name;

    #[link(name = "ServiceManagement", kind = "framework")]
    extern "C" {}

    /// In `Contents/Library/LaunchDaemons`.
    const PLIST: &str = "com.zwiim.power-helper.plist";
    const ENABLED: isize = 1;
    const REQUIRES_APPROVAL: isize = 2;

    pub fn setup() -> Result<String, String> {
        let class = AnyClass::get(c"SMAppService").ok_or("ServiceManagement unavailable")?;
        let name = NSString::from_str(PLIST);
        // SAFETY: documented SMAppService API (macOS 13+), argument types as declared.
        let service: Option<Retained<AnyObject>> =
            unsafe { msg_send![class, daemonServiceWithPlistName: &*name] };
        let service = service.ok_or("power helper not bundled")?;
        let status = |service: &AnyObject| -> isize { unsafe { msg_send![service, status] } };
        if status(&service) == ENABLED {
            return Ok(status_name(ENABLED).into());
        }
        let registered: Result<(), Retained<NSError>> =
            unsafe { msg_send![&*service, registerAndReturnError: _] };
        let now = status(&service);
        if now == REQUIRES_APPROVAL {
            let _: () = unsafe { msg_send![class, openSystemSettingsLoginItems] };
        }
        match registered {
            Err(e) if now != ENABLED && now != REQUIRES_APPROVAL => {
                Err(e.localizedDescription().to_string())
            }
            _ => Ok(status_name(now).into()),
        }
    }
}

#[cfg(not(target_os = "macos"))]
mod imp {
    pub fn setup() -> Result<String, String> {
        Err("unsupported".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_names() {
        assert_eq!(status_name(0), "not_registered");
        assert_eq!(status_name(1), "enabled");
        assert_eq!(status_name(2), "requires_approval");
        assert_eq!(status_name(3), "not_found");
        assert_eq!(status_name(4), "unknown");
        assert_eq!(status_name(-1), "unknown");
    }
}
