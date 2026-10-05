//! The secure attention (Ctrl-Alt-Del) screen.

use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS};
use windows::Win32::Security::Authentication::Identity::SendSAS;
use windows::Win32::System::Registry::{HKEY_LOCAL_MACHINE, RRF_RT_REG_DWORD, RegGetValueW};
use windows::core::PCWSTR;

use crate::handle::wide;

/// The policy key holding [`SAS_POLICY_VALUE`], under `HKEY_LOCAL_MACHINE`.
pub const SAS_POLICY_KEY: &str = r"SOFTWARE\Microsoft\Windows\CurrentVersion\Policies\System";
/// 0 none, 1 services, 2 ease-of-access applications, 3 both.
pub const SAS_POLICY_VALUE: &str = "SoftwareSASGeneration";

/// The software secure attention policy, or `None` if it is not set.
pub fn sas_policy() -> std::io::Result<Option<u32>> {
    let key = wide(SAS_POLICY_KEY);
    let value = wide(SAS_POLICY_VALUE);
    let mut data = 0u32;
    let mut size = size_of::<u32>() as u32;
    // SAFETY: both names are NUL-terminated; `data` holds `size` bytes.
    let e = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            PCWSTR(key.as_ptr()),
            PCWSTR(value.as_ptr()),
            RRF_RT_REG_DWORD,
            None,
            Some((&raw mut data).cast()),
            Some(&mut size),
        )
    };
    match e {
        ERROR_SUCCESS => Ok(Some(data)),
        ERROR_FILE_NOT_FOUND => Ok(None),
        e => Err(std::io::Error::from_raw_os_error(e.0 as i32)),
    }
}

/// Whether `policy` lets services generate secure attention.
pub fn allows_services(policy: Option<u32>) -> bool {
    matches!(policy, Some(1 | 3))
}

/// Asks Windows to show the secure attention screen, as a service. Windows
/// reports nothing back; without the policy it silently does nothing.
pub fn send_sas() {
    // SAFETY: no pointers; `false` means the caller is a service.
    unsafe { SendSAS(false) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn policy_values() {
        assert!(!allows_services(None));
        assert!(!allows_services(Some(0)));
        assert!(allows_services(Some(1)));
        assert!(!allows_services(Some(2)));
        assert!(allows_services(Some(3)));
    }

    #[test]
    fn reads_this_machines_policy() {
        sas_policy().unwrap();
    }
}
