use std::ffi::CStr;
use std::fs::{read_dir, remove_dir_all, remove_file, remove_dir, copy, create_dir};
use std::fs::FileType;
use std::ptr::null;
use std::path::Path;

use libc::{c_int, uid_t, gid_t, c_char, c_void, timeval};
use libc::chmod;

use super::root::temporary_change_root;

pub type Time = f64;

// pwd.h
#[repr(C)]
struct passwd {
    pw_name: *mut c_char,       /* username */
    pw_passwd: *mut u8,     /* user password */
    pw_uid: uid_t,      /* user ID */
    pw_gid: gid_t,      /* group ID */
    pw_gecos: *mut u8,      /* user information */
    pw_dir: *mut u8,        /* home directory */
    pw_shell: *mut u8,      /* shell program */
}

extern "C" {
    // pwd.h
    fn getpwuid(uid: uid_t) -> *const passwd;
    // <sys/time.h>
    fn gettimeofday(time: *mut timeval, tz: *const c_void) -> c_int;
}

pub fn get_user_name(uid: uid_t) -> Result<String, String> {
    unsafe {
        let val = getpwuid(uid);
        if val != null() {
            return Ok(String::from_utf8_lossy(
                CStr.from_ptr((*val).pw_name).to_bytes()));
        }
    }
    return Err(format!("User {} not found", uid));
}

pub fn clean_dir(dir: &Path, remove_dir_itself: bool) -> Result<(), String> {
    if !dir.exists() {
        return Ok(());
    }
    // We temporarily change root, so that symlinks inside the dir
    // would do no harm. But note that dir itself can be a symlink
    try!(temporary_change_root(dir, || {
        let diriter = try!(read_dir(&Path::new("/"))
             .map_err(|e| format!("Can't read directory {}: {}",
                                  dir.display(), e)));
        for entry in diriter {
            let entry = try_msg!(entry, "Error reading dir entry: {err}");
            if entry.is_dir() {
                try_msg!(remove_dir_all(&entry.path()),
                    "Can't remove directory {dir:?}: {err}", dir=entry.path());
            } else {
                try_msg!(remove_file(&path),
                    "Can't remove file {dir:?}: {err}", dir=entry.path());
            }
        }
        Ok(())
    }));
    if remove_dir_itself {
        try!(rmdir(dir).map_err(|e| format!("Can't remove dir {}: {}",
                                            dir.display(), e)));
    }
    return Ok(());
}

pub fn get_time() -> Time {
    let mut tv = timeval { tv_sec: 0, tv_usec: 0 };
    unsafe { gettimeofday(&mut tv, null()) };
    return tv.tv_sec as f64 + 0.000001 * tv.tv_usec as f64;
}

pub fn copy_dir(old: &Path, new: &Path) -> Result<(), String> {
    // TODO(tailhook) use reflinks if supported
    let filelist = try!(readdir(old)
        .map_err(|e| format!("Error reading directory: {}", e)));
    for item in filelist.iter() {
        let stat = try!(item.lstat()
            .map_err(|e| format!("Error stat for file: {}", e)));
        let nitem = new.join(item.filename().unwrap());
        match stat.kind {
            FileType::RegularFile => {
                try!(copy(item, &nitem)
                    .map_err(|e| format!("Can't hard-link file: {}", e)));
            }
            FileType::Directory => {
                if !nitem.is_dir() {
                    try_msg!(create_dir_perm(&nitem, stat.perm),
                        "Can't create dir {dir:?}: {err}", dir=nitem);
                }
                try!(copy_dir(item, &nitem));
            }
            FileType::NamedPipe => {
                warn!("Skipping named pipe {:?}", item);
            }
            FileType::BlockSpecial => {
                warn!("Can't clone block-special {:?}, skipping", item);
            }
            FileType::Symlink => {
                let lnk = try!(readlink(item)
                    .map_err(|e| format!("Can't readlink: {}", e)));
                try!(symlink(&lnk, &nitem)
                    .map_err(|e| format!("Can't symlink: {}", e)));
            }
            FileType::Unknown => {
                warn!("Unknown file type {:?}", item);
            }
        }
    }
    Ok(())
}
