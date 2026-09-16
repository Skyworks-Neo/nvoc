//! OS-account authentication for the HTTP control plane.
//!
//! HTTP Basic credentials are checked against the operating system's own
//! account database — no passwords are stored or maintained here:
//! - Windows: `LogonUserW` + membership check against BUILTIN\Administrators.
//! - Linux: `/etc/shadow` verify via `pwhash` (the service runs as root) +
//!   membership in any configured group (default wheel/sudo).
//!
//! Hash-format guard: yescrypt (`$y$`/`$gy$`) is not supported by `pwhash`;
//! such a hash yields an explicit configuration error, never a silent
//! rejection of a valid password.

use crate::config::RuntimeConfig;

#[derive(Debug, Clone)]
pub struct Credentials {
    pub user: String,
    pub password: String,
}

/// Decode an `Authorization: Basic ...` header value.
pub fn parse_basic(header_value: &str) -> Option<Credentials> {
    let encoded = header_value
        .strip_prefix("Basic ")
        .or_else(|| header_value.strip_prefix("basic "))?;
    let decoded = base64_decode(encoded.trim())?;
    let text = String::from_utf8(decoded).ok()?;
    let (user, password) = text.split_once(':')?;
    Some(Credentials {
        user: user.to_string(),
        password: password.to_string(),
    })
}

/// Minimal standard-base64 decoder (ASCII, padding tolerated).
fn base64_decode(input: &str) -> Option<Vec<u8>> {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = Vec::with_capacity(input.len() * 3 / 4);
    let mut acc: u32 = 0;
    let mut bits: u32 = 0;
    for ch in input.bytes() {
        if ch == b'=' {
            break;
        }
        if matches!(ch, b'\r' | b'\n' | b' ') {
            continue;
        }
        let v = TABLE.iter().position(|t| *t == ch)? as u32;
        acc = (acc << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    Some(out)
}

/// Verify credentials against the OS account database and require
/// administrator-group membership. `Err` carries a user-facing reason.
pub fn verify(user: &str, password: &str, cfg: &RuntimeConfig) -> std::result::Result<(), String> {
    os_verify(user, password, cfg)
}

#[cfg(windows)]
fn os_verify(user: &str, password: &str, _cfg: &RuntimeConfig) -> std::result::Result<(), String> {
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::Security::LogonUserW;
    use windows_sys::Win32::Security::{
        AllocateAndInitializeSid, EqualSid, FreeSid, GetTokenInformation, SID_AND_ATTRIBUTES,
        SID_IDENTIFIER_AUTHORITY, TOKEN_GROUPS, TokenGroups,
    };

    const LOGON32_LOGON_INTERACTIVE: u32 = 2;
    const LOGON32_PROVIDER_DEFAULT: u32 = 0;
    // SE_GROUP_ENABLED (used) — and SE_GROUP_USE_FOR_DENY_ONLY, which is how
    // the UAC-filtered token of an *administrator account* carries the
    // Administrators SID. Deny-only therefore still proves "this account is
    // an admin"; the filtered token itself is not elevated, but that is not
    // what we are asking.
    const SE_GROUP_ENABLED: u32 = 0x0000_0004;
    const SE_GROUP_USE_FOR_DENY_ONLY: u32 = 0x0000_0010;

    fn to_wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    unsafe fn admin_sid() -> Result<*mut core::ffi::c_void, String> {
        unsafe {
            let authority = SID_IDENTIFIER_AUTHORITY {
                Value: [0, 0, 0, 0, 0, 5],
            };
            let mut sid: *mut core::ffi::c_void = std::ptr::null_mut();
            let ok = AllocateAndInitializeSid(
                &authority, 1, 544, // DOMAIN_ALIAS_RID_ADMINS
                0, 0, 0, 0, 0, 0, 0, &mut sid,
            );
            if ok == 0 || sid.is_null() {
                return Err("cannot construct the Administrators SID".to_string());
            }
            Ok(sid)
        }
    }

    /// Admin membership from a token's group list: the Administrators SID
    /// present with either the enabled or the deny-only attribute.
    unsafe fn token_is_admin(token: HANDLE, admin: *mut core::ffi::c_void) -> bool {
        unsafe {
            let mut len: u32 = 0;
            GetTokenInformation(token, TokenGroups, std::ptr::null_mut(), 0, &mut len);
            if len == 0 {
                return false;
            }
            let mut buf = vec![0u8; len as usize];
            let ok =
                GetTokenInformation(token, TokenGroups, buf.as_mut_ptr().cast(), len, &mut len);
            if ok == 0 {
                return false;
            }
            let groups = &*(buf.as_ptr() as *const TOKEN_GROUPS);
            for i in 0..groups.GroupCount as usize {
                let g = groups.Groups.get(i).unwrap_or(&SID_AND_ATTRIBUTES {
                    Sid: std::ptr::null_mut(),
                    Attributes: 0,
                });
                if g.Sid.is_null() {
                    continue;
                }
                if EqualSid(g.Sid, admin) != 0 {
                    let attrs = g.Attributes;
                    return attrs & (SE_GROUP_ENABLED | SE_GROUP_USE_FOR_DENY_ONLY) != 0;
                }
            }
            false
        }
    }

    let user_w = to_wide(user);
    let pass_w = to_wide(password);

    unsafe {
        let mut token: HANDLE = std::ptr::null_mut();
        let ok = LogonUserW(
            user_w.as_ptr(),
            std::ptr::null(), // default domain: local machine first
            pass_w.as_ptr(),
            LOGON32_LOGON_INTERACTIVE,
            LOGON32_PROVIDER_DEFAULT,
            &mut token,
        );
        if ok == 0 {
            return Err("invalid username or password".to_string());
        }

        let admin = match admin_sid() {
            Ok(sid) => sid,
            Err(e) => {
                CloseHandle(token);
                return Err(e);
            }
        };
        let is_admin = token_is_admin(token, admin);
        FreeSid(admin);
        CloseHandle(token);
        if !is_admin {
            return Err(format!(
                "{user} is not an administrator account (UAC deny-only count included)"
            ));
        }
        Ok(())
    }
}

#[cfg(not(windows))]
fn os_verify(user: &str, password: &str, cfg: &RuntimeConfig) -> std::result::Result<(), String> {
    // /etc/shadow is root-readable only — the unit runs as root.
    let shadow = std::fs::read_to_string("/etc/shadow")
        .map_err(|_| "auth=on requires root (cannot read /etc/shadow)".to_string())?;
    let mut hash: Option<String> = None;
    for line in shadow.lines() {
        let mut fields = line.split(':');
        if fields.next() == Some(user) {
            hash = fields.next().map(|h| h.to_string());
            break;
        }
    }
    let hash = hash
        .filter(|h| !h.is_empty() && h != "!" && h != "*")
        .ok_or_else(|| format!("user {user:?} has no usable password hash"))?;
    if hash.starts_with("$y$") || hash.starts_with("$gy$") {
        return Err(
            "yescrypt hashes are not supported by the shadow verifier; keep auth=off \
             or switch the account to sha512crypt"
                .to_string(),
        );
    }
    let ok =
        pwhash::unix::verify(password, &hash).map_err(|e| format!("hash verify failed: {e}"))?;
    if !ok {
        return Err("invalid username or password".to_string());
    }

    // Group membership: any configured group (wheel/sudo), direct or primary.
    let allowed_groups: Vec<String> = cfg
        .auth_group
        .split(',')
        .map(|g| g.trim().to_string())
        .filter(|g| !g.is_empty())
        .collect();
    if allowed_groups.is_empty() {
        return Ok(());
    }
    let group_db = std::fs::read_to_string("/etc/group").unwrap_or_default();
    let mut allowed_gids: Vec<u32> = Vec::new();
    for line in group_db.lines() {
        let fields: Vec<&str> = line.split(':').collect();
        if fields.len() >= 4 {
            let name = fields[0];
            if allowed_groups.iter().any(|g| g == name) {
                if let Ok(gid) = fields[2].parse::<u32>() {
                    allowed_gids.push(gid);
                }
                if fields[3].split(',').any(|m| m == user) {
                    return Ok(());
                }
            }
        }
    }
    let passwd = std::fs::read_to_string("/etc/passwd").unwrap_or_default();
    for line in passwd.lines() {
        let fields: Vec<&str> = line.split(':').collect();
        if fields.len() >= 4 && fields[0] == user {
            if let Ok(gid) = fields[3].parse::<u32>() {
                if allowed_gids.contains(&gid) {
                    return Ok(());
                }
            }
        }
    }
    Err(format!(
        "{user} is not in the allowed groups ({})",
        cfg.auth_group
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic_header_parsing() {
        let c = parse_basic("Basic dXNlcjpwYXNz").expect("decode"); // user:pass
        assert_eq!(c.user, "user");
        assert_eq!(c.password, "pass");
        assert!(parse_basic("Bearer xyz").is_none());
        assert!(parse_basic("Basic !!!not-base64!!!").is_none());
        assert!(parse_basic("Basic").is_none());
    }
}
