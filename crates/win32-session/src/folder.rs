//! The service's data folder: owned by SYSTEM, open to SYSTEM only.

use std::io;
use std::os::windows::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use windows::Win32::Foundation::{
    ERROR_ALREADY_EXISTS, ERROR_FILE_EXISTS, ERROR_FILE_NOT_FOUND, ERROR_PATH_NOT_FOUND,
    WIN32_ERROR,
};
use windows::Win32::Security::Authorization::{
    ConvertSidToStringSidW, GetNamedSecurityInfoW, SE_FILE_OBJECT, SetNamedSecurityInfoW,
};
use windows::Win32::Security::{
    DACL_SECURITY_INFORMATION, GetSecurityDescriptorDacl, GetSecurityDescriptorOwner,
    OWNER_SECURITY_INFORMATION, PROTECTED_DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID,
};
use windows::Win32::Storage::FileSystem::{
    CREATE_NEW, CreateDirectoryW, CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_ATTRIBUTE_REPARSE_POINT,
    FILE_GENERIC_WRITE, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
};
use windows::core::{PCWSTR, PWSTR};

use crate::handle::{Local, wide};
use crate::pipe::Descriptor;
use crate::sid::{Sid, is_trusted_owner};

/// Owner SYSTEM; SYSTEM has full control, inherited by everything inside;
/// nothing is inherited from the parent.
const FOLDER_SDDL: &str = "O:SYD:P(A;OICI;FA;;;SY)";

/// What [`secure_folder`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Prepared {
    /// The folder did not exist and was created.
    Created,
    /// It existed with a trusted owner; its owner and ACL were reset.
    Repaired,
    /// It existed with another owner, or was a link, and was moved to
    /// `moved_to`; a new folder was created.
    Replaced { moved_to: PathBuf },
}

/// Makes `path` a folder owned by SYSTEM that only SYSTEM can open, before
/// anything in it is read. Needs SYSTEM.
///
/// An existing folder owned by SYSTEM or Administrators is kept, and its
/// owner and ACL are reset (repairing what its children inherit). Anything
/// else at `path` (another owner, a junction or symbolic link, a file) is
/// moved aside unread and replaced.
pub fn secure_folder(path: &Path) -> io::Result<Prepared> {
    for _ in 0..3 {
        match std::fs::symlink_metadata(path) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => match create(path) {
                Ok(()) => return Ok(Prepared::Created),
                // Someone created it in the meantime: check it again.
                Err(e) if e.raw_os_error() == Some(ERROR_ALREADY_EXISTS.0 as i32) => continue,
                Err(e) => return Err(e),
            },
            Err(e) => return Err(e),
            Ok(meta) => {
                let link = meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT.0 != 0;
                if !link && meta.is_dir() && is_trusted_owner(&owner(path)?) {
                    repair(path)?;
                    return Ok(Prepared::Repaired);
                }
                let moved_to = aside(path);
                std::fs::rename(path, &moved_to)?;
                create(path)?;
                return Ok(Prepared::Replaced { moved_to });
            }
        }
    }
    Err(io::Error::other(
        "the data folder keeps changing while it is prepared",
    ))
}

/// What [`remove_secure_folder`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Removal {
    /// A trusted folder, removed with everything in it.
    Removed,
    /// Nothing was at `path`.
    Absent,
    /// Something else was at `path`, left untouched, and why.
    Refused(String),
}

/// Removes the data folder `path` and everything in it, but only if it is a
/// real folder owned by SYSTEM or Administrators, the folders
/// [`secure_folder`] keeps (add-installer design D4). A junction, symbolic
/// link or file at `path`, or a folder with another owner, is left
/// untouched, and so is anything it points to.
///
/// Links inside the folder are removed, never followed: `remove_dir_all`
/// opens each entry without following reparse points. Only SYSTEM and
/// Administrators can replace a folder they own, so nobody else can swap it
/// for a link between the check and the removal.
pub fn remove_secure_folder(path: &Path) -> io::Result<Removal> {
    let meta = match std::fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Removal::Absent),
        Err(e) => return Err(e),
    };
    if meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT.0 != 0 {
        return Ok(Removal::Refused("it is a link or junction".into()));
    }
    if !meta.is_dir() {
        return Ok(Removal::Refused("it is not a folder".into()));
    }
    let owner = owner(path)?;
    if !is_trusted_owner(&owner) {
        return Ok(Removal::Refused(format!(
            "it is owned by {owner}, not SYSTEM or Administrators"
        )));
    }
    std::fs::remove_dir_all(path)?;
    Ok(Removal::Removed)
}

/// `<name>.untrusted-<unix seconds>` next to `path`.
fn aside(path: &Path) -> PathBuf {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    path.with_file_name(format!("{name}.untrusted-{secs}"))
}

fn create(path: &Path) -> io::Result<()> {
    let descriptor = Descriptor::from_sddl(FOLDER_SDDL)?;
    let attributes = descriptor.attributes();
    let name = wide(path);
    // SAFETY: `name` is NUL-terminated; `attributes` points at a descriptor
    // that outlives the call.
    unsafe { CreateDirectoryW(PCWSTR(name.as_ptr()), Some(&attributes)) }?;
    Ok(())
}

fn win32(e: WIN32_ERROR) -> io::Result<()> {
    if e.is_ok() {
        Ok(())
    } else {
        Err(io::Error::from_raw_os_error(e.0 as i32))
    }
}

/// The owner of the file or folder at `path`.
fn owner(path: &Path) -> io::Result<Sid> {
    let name = wide(path);
    let mut owner = PSID::default();
    let mut sd = PSECURITY_DESCRIPTOR::default();
    // SAFETY: `name` is NUL-terminated; `sd` receives LocalAlloc memory
    // that `Local` frees, and `owner` points into it.
    let e = unsafe {
        GetNamedSecurityInfoW(
            PCWSTR(name.as_ptr()),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION,
            Some(&mut owner),
            None,
            None,
            None,
            &mut sd,
        )
    };
    let _free = Local(sd.0);
    if e == ERROR_FILE_NOT_FOUND || e == ERROR_PATH_NOT_FOUND {
        return Err(io::ErrorKind::NotFound.into());
    }
    win32(e)?;
    sid_text(owner)
}

fn sid_text(sid: PSID) -> io::Result<Sid> {
    let mut text = PWSTR::null();
    // SAFETY: `sid` is valid for the caller's borrow; `text` receives
    // LocalAlloc memory that `Local` frees.
    unsafe { ConvertSidToStringSidW(sid, &mut text) }?;
    let _free = Local(text.0.cast());
    // SAFETY: on success `text` is a NUL-terminated UTF-16 string.
    let s = unsafe { text.to_string() }.map_err(|_| io::Error::other("SID is not UTF-16"))?;
    Sid::parse(&s).ok_or_else(|| io::Error::other(format!("unexpected SID form {s:?}")))
}

/// Sets owner SYSTEM and the protected DACL on an existing folder; Windows
/// re-applies inheritable entries to what is inside.
fn repair(path: &Path) -> io::Result<()> {
    let descriptor = Descriptor::from_sddl(FOLDER_SDDL)?;
    let sd = PSECURITY_DESCRIPTOR(descriptor.attributes().lpSecurityDescriptor);
    let mut owner = PSID::default();
    let mut defaulted = false.into();
    // SAFETY: `sd` is a valid self-relative descriptor; `owner` points into it.
    unsafe { GetSecurityDescriptorOwner(sd, &mut owner, &mut defaulted) }?;
    let mut present = false.into();
    let mut dacl = std::ptr::null_mut();
    // SAFETY: as above; `dacl` points into `sd`.
    unsafe { GetSecurityDescriptorDacl(sd, &mut present, &mut dacl, &mut defaulted) }?;
    let name = wide(path);
    // SAFETY: `name` is NUL-terminated; `owner` and `dacl` point into a
    // descriptor that outlives the call.
    let e = unsafe {
        SetNamedSecurityInfoW(
            PCWSTR(name.as_ptr()),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION
                | DACL_SECURITY_INFORMATION
                | PROTECTED_DACL_SECURITY_INFORMATION,
            Some(owner),
            None,
            Some(dacl),
            None,
        )
    };
    win32(e)
}

/// SYSTEM has full control and Administrators may read; protected, so the
/// data folder's SYSTEM-only entry is not inherited.
const LOG_SDDL: &str = "D:P(A;;FA;;;SY)(A;;FR;;;BA)";

/// Creates the empty file `path`, readable by Administrators, if it does
/// not exist; an existing file is left as it is. For the service's log, so
/// an elevated administrator can read it inside the SYSTEM-only folder
/// (traversing a folder needs no access to it).
pub fn create_admin_readable(path: &Path) -> io::Result<()> {
    let descriptor = Descriptor::from_sddl(LOG_SDDL)?;
    let attributes = descriptor.attributes();
    let name = wide(path);
    // SAFETY: `name` is NUL-terminated; `attributes` points at a descriptor
    // that outlives the call; the handle returned is owned and closed.
    let created = unsafe {
        CreateFileW(
            PCWSTR(name.as_ptr()),
            FILE_GENERIC_WRITE.0,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            Some(&attributes),
            CREATE_NEW,
            FILE_ATTRIBUTE_NORMAL,
            None,
        )
    };
    match created {
        Ok(handle) => {
            drop(crate::handle::Owned(handle));
            Ok(())
        }
        Err(e) if e.code() == ERROR_FILE_EXISTS.to_hresult() => Ok(()),
        Err(e) => Err(e.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folder_descriptor_parses() {
        Descriptor::from_sddl(FOLDER_SDDL).unwrap();
        Descriptor::from_sddl(LOG_SDDL).unwrap();
    }

    #[test]
    fn admin_readable_file_is_created_once() {
        let dir = std::env::temp_dir().join(format!("deskhop-logacl-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("service.log");
        let _ = std::fs::remove_file(&path);
        create_admin_readable(&path).unwrap();
        assert_eq!(std::fs::metadata(&path).unwrap().len(), 0);
        // A second call leaves the existing file alone.
        create_admin_readable(&path).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn aside_name_keeps_the_folder_name() {
        let moved = aside(Path::new(r"C:\ProgramData\DeskHop"));
        let name = moved.file_name().unwrap().to_string_lossy().into_owned();
        assert!(name.starts_with("DeskHop.untrusted-"), "{name}");
        assert_eq!(moved.parent(), Some(Path::new(r"C:\ProgramData")));
    }

    fn temp(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("deskhop-remove-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn removing_an_absent_folder() {
        let dir = temp("absent");
        assert_eq!(remove_secure_folder(&dir).unwrap(), Removal::Absent);
    }

    #[test]
    fn a_junction_is_refused_and_its_target_kept() {
        let base = temp("junction");
        let target = base.join("target");
        let link = base.join("DeskHop");
        std::fs::create_dir_all(&target).unwrap();
        std::fs::write(target.join("keep.txt"), b"keep").unwrap();
        // Junctions need no privilege, unlike symbolic links.
        let made = std::process::Command::new("cmd")
            .args(["/c", "mklink", "/J"])
            .arg(&link)
            .arg(&target)
            .output()
            .unwrap();
        assert!(made.status.success(), "{made:?}");

        let removal = remove_secure_folder(&link).unwrap();
        assert!(matches!(removal, Removal::Refused(_)), "{removal:?}");
        assert!(
            std::fs::symlink_metadata(&link).is_ok(),
            "the junction was removed"
        );
        assert_eq!(std::fs::read(target.join("keep.txt")).unwrap(), b"keep");

        std::fs::remove_dir(&link).unwrap();
        std::fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn a_file_is_refused() {
        let base = temp("file");
        std::fs::create_dir_all(&base).unwrap();
        let file = base.join("DeskHop");
        std::fs::write(&file, b"x").unwrap();
        let removal = remove_secure_folder(&file).unwrap();
        assert!(matches!(removal, Removal::Refused(_)), "{removal:?}");
        assert!(file.exists());
        std::fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn a_folder_is_removed_only_with_a_trusted_owner() {
        let dir = temp("owner");
        std::fs::create_dir_all(dir.join("inner")).unwrap();
        std::fs::write(dir.join("inner").join("f"), b"x").unwrap();
        // A normal user owns what it creates, so it is refused; an elevated
        // administrator (CI) creates folders owned by Administrators, which
        // are trusted, so it is removed.
        let trusted = is_trusted_owner(&owner(&dir).unwrap());
        let removal = remove_secure_folder(&dir).unwrap();
        if trusted {
            assert_eq!(removal, Removal::Removed);
            assert!(!dir.exists());
        } else {
            assert!(matches!(removal, Removal::Refused(_)), "{removal:?}");
            assert!(dir.join("inner").join("f").exists());
            std::fs::remove_dir_all(&dir).unwrap();
        }
    }

    #[test]
    fn owner_of_a_file_this_user_created_is_this_user() {
        let dir = std::env::temp_dir().join(format!("deskhop-owner-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let me = crate::own_sid().unwrap();
        let found = owner(&dir).unwrap();
        // Elevated administrators create files owned by Administrators.
        assert!(
            found == me || found.as_str() == crate::sid::ADMINISTRATORS,
            "{found}"
        );
        std::fs::remove_dir(&dir).unwrap();
        assert_eq!(owner(&dir).unwrap_err().kind(), io::ErrorKind::NotFound);
    }
}
