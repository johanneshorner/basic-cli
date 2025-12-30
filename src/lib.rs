//! Roc platform host implementation for basic-cli using the new RocOps-based ABI.

use std::ffi::c_void;
use std::fs;
use std::io::{self, BufRead, Write};
use std::sync::atomic::{AtomicBool, Ordering};

use roc_std_new::{
    HostedFn, HostedFunctions, RocAlloc, RocCrashed, RocDbg, RocDealloc, RocExpectFailed,
    RocList, RocOps, RocRealloc, RocStr,
};

/// Roc Try/Result type layout:
/// - Payload at offset 0 (sized to max of Ok/Err payloads)
/// - Discriminant immediately after payload (no padding for discriminant)
/// - Err = 0, Ok = 1 (alphabetically sorted)
///
/// IMPORTANT: Single-variant tag unions like [PathErr(IOErr)] do NOT elide
/// their discriminant in Roc. Use `RocSingleTagWrapper<T>` to represent them.
#[repr(C)]
pub struct RocTry<T, E> {
    payload: RocTryPayload<T, E>,
    discriminant: u8,
}

#[repr(C)]
pub union RocTryPayload<T, E> {
    ok: core::mem::ManuallyDrop<T>,
    err: core::mem::ManuallyDrop<E>,
}

impl<T, E> RocTry<T, E> {
    /// Create an Ok result (discriminant = 1)
    pub fn ok(value: T) -> Self {
        Self {
            payload: RocTryPayload {
                ok: core::mem::ManuallyDrop::new(value),
            },
            discriminant: 1, // Ok = 1
        }
    }

    /// Create an Err result (discriminant = 0)
    pub fn err(value: E) -> Self {
        Self {
            payload: RocTryPayload {
                err: core::mem::ManuallyDrop::new(value),
            },
            discriminant: 0, // Err = 0
        }
    }
}

/// Wrapper for single-variant tag unions like [PathErr(IOErr)].
///
/// In Roc, single-variant tag unions do NOT elide their discriminant.
/// This wrapper adds the discriminant byte that Roc expects.
///
/// Example: `[PathErr(IOErr)]` in Roc = `RocSingleTagWrapper<IOErr>` in Rust
#[repr(C)]
pub struct RocSingleTagWrapper<T> {
    pub payload: T,
    discriminant: u8, // Always 0 for single-variant
}

impl<T> RocSingleTagWrapper<T> {
    pub fn new(payload: T) -> Self {
        Self {
            payload,
            discriminant: 0, // Single variant is always index 0
        }
    }
}

/// Global flag to track if dbg or expect_failed was called.
/// If set, program exits with non-zero code to prevent accidental commits.
static DEBUG_OR_EXPECT_CALLED: AtomicBool = AtomicBool::new(false);

// External symbol provided by the compiled Roc application
extern "C" {
    fn roc__main_for_host(ops: *const RocOps, ret_ptr: *mut c_void, args_ptr: *mut c_void);
}

/// Roc allocation function with size-tracking metadata.
///
/// We store the allocation size before the user data so we can properly
/// deallocate later (since RocDealloc doesn't provide the size).
extern "C" fn roc_alloc_fn(roc_alloc: *mut RocAlloc, _env: *mut c_void) {
    unsafe {
        let args = &mut *roc_alloc;

        // Sanity check - if length is absurdly large, something is wrong
        if args.length > 1024 * 1024 * 1024 {
            eprintln!("\x1b[31mHost error:\x1b[0m allocation failed - length too large");
            eprintln!("  alignment={}, length={}", args.alignment, args.length);
            std::process::exit(1);
        }

        // Ensure alignment is at least 1 and a power of 2
        let alignment = args.alignment.max(1);
        let min_alignment = alignment.max(std::mem::align_of::<usize>());

        // Ensure min_alignment is a power of 2
        let min_alignment = min_alignment.next_power_of_two();

        // Calculate additional bytes needed to store the size
        let size_storage_bytes = min_alignment;
        let total_size = args.length.saturating_add(size_storage_bytes);

        // Ensure total_size is at least 1
        let total_size = total_size.max(1);

        // Use libc malloc directly for more reliable allocation
        let base_ptr = libc::malloc(total_size) as *mut u8;

        if base_ptr.is_null() {
            eprintln!("\x1b[31mHost error:\x1b[0m allocation failed, out of memory");
            eprintln!(
                "  requested: alignment={}, length={}",
                args.alignment, args.length
            );
            eprintln!(
                "  computed: min_alignment={}, size_storage_bytes={}, total_size={}",
                min_alignment, size_storage_bytes, total_size
            );
            std::process::exit(1);
        }

        // Store the total size right before the user data
        let size_ptr =
            base_ptr.add(size_storage_bytes - std::mem::size_of::<usize>()) as *mut usize;
        *size_ptr = total_size;

        // Also store the alignment for deallocation
        // We use the first usize slot for alignment, second for total_size
        if size_storage_bytes >= 2 * std::mem::size_of::<usize>() {
            let align_ptr = base_ptr as *mut usize;
            *align_ptr = min_alignment;
        }

        // Return pointer to the user data (after the size metadata)
        args.answer = base_ptr.add(size_storage_bytes) as *mut c_void;
    }
}

/// Roc deallocation function with size-tracking metadata.
extern "C" fn roc_dealloc_fn(roc_dealloc: *mut RocDealloc, _env: *mut c_void) {
    unsafe {
        let args = &*roc_dealloc;

        // Use the same alignment calculation as alloc
        let alignment = args.alignment.max(1);
        let min_alignment = alignment
            .max(std::mem::align_of::<usize>())
            .next_power_of_two();
        let size_storage_bytes = min_alignment;

        // Calculate the base pointer (start of actual allocation)
        let base_ptr = (args.ptr as *mut u8).sub(size_storage_bytes);

        // Free the memory using libc
        libc::free(base_ptr as *mut c_void);
    }
}

/// Roc reallocation function with size-tracking metadata.
extern "C" fn roc_realloc_fn(roc_realloc: *mut RocRealloc, _env: *mut c_void) {
    unsafe {
        let args = &mut *roc_realloc;

        // Use the same alignment calculation as alloc
        let alignment = args.alignment.max(1);
        let min_alignment = alignment
            .max(std::mem::align_of::<usize>())
            .next_power_of_two();
        let size_storage_bytes = min_alignment;

        // Get old allocation info
        let old_base_ptr = (args.answer as *mut u8).sub(size_storage_bytes);

        // Calculate new total size
        let new_total_size = args.new_length.saturating_add(size_storage_bytes).max(1);

        // Use libc realloc
        let new_base_ptr = libc::realloc(old_base_ptr as *mut c_void, new_total_size) as *mut u8;

        if new_base_ptr.is_null() {
            eprintln!("\x1b[31mHost error:\x1b[0m reallocation failed, out of memory");
            std::process::exit(1);
        }

        // Store the new total size in metadata
        let new_size_ptr =
            new_base_ptr.add(size_storage_bytes - std::mem::size_of::<usize>()) as *mut usize;
        *new_size_ptr = new_total_size;

        // Return pointer to the user data
        args.answer = new_base_ptr.add(size_storage_bytes) as *mut c_void;
    }
}

/// Roc debug function - called when Roc code uses `dbg`.
extern "C" fn roc_dbg_fn(roc_dbg: *const RocDbg, _env: *mut c_void) {
    DEBUG_OR_EXPECT_CALLED.store(true, Ordering::Release);
    unsafe {
        let args = &*roc_dbg;
        let message = std::slice::from_raw_parts(args.utf8_bytes, args.len);
        let message = std::str::from_utf8_unchecked(message);
        eprintln!("\x1b[33mdbg:\x1b[0m {}", message);
    }
}

/// Roc expect failed function - called when an `expect` statement fails.
extern "C" fn roc_expect_failed_fn(roc_expect: *const RocExpectFailed, _env: *mut c_void) {
    DEBUG_OR_EXPECT_CALLED.store(true, Ordering::Release);
    unsafe {
        let args = &*roc_expect;
        let message = std::slice::from_raw_parts(args.utf8_bytes, args.len);
        let message = std::str::from_utf8_unchecked(message).trim();
        eprintln!("\x1b[33mexpect failed:\x1b[0m {}", message);
    }
}

/// Roc crashed function - called when the Roc program crashes.
extern "C" fn roc_crashed_fn(roc_crashed: *const RocCrashed, _env: *mut c_void) {
    unsafe {
        let args = &*roc_crashed;
        let message = std::slice::from_raw_parts(args.utf8_bytes, args.len);
        let message = std::str::from_utf8_unchecked(message);
        eprintln!("\n\x1b[31mRoc crashed:\x1b[0m {}", message);
        std::process::exit(1);
    }
}

// ============================================================================
// Hosted Functions (sorted alphabetically by fully-qualified name)
// ============================================================================

/// Hosted function: Dir.create! (index 0)
/// Takes Str, returns Try({}, [DirErr(IOErr)])
extern "C" fn hosted_dir_create(
    ops: *const RocOps,
    ret_ptr: *mut c_void,
    args_ptr: *mut c_void,
) {
    let roc_ops = unsafe { &*ops };
    let result = unsafe {
        let path = args_ptr as *const RocStr;
        fs::create_dir((*path).as_str())
    };
    let try_result: TryUnitDirErr = match result {
        Ok(()) => RocTry::ok(()),
        Err(e) => {
            let io_err = roc_io_error::IOErr::from_io_error(&e, roc_ops);
            RocTry::err(RocSingleTagWrapper::new(io_err))
        }
    };
    unsafe {
        std::ptr::write(ret_ptr as *mut TryUnitDirErr, try_result);
    }
}

/// Hosted function: Dir.create_all! (index 1)
/// Takes Str, returns Try({}, [DirErr(IOErr)])
extern "C" fn hosted_dir_create_all(
    ops: *const RocOps,
    ret_ptr: *mut c_void,
    args_ptr: *mut c_void,
) {
    let roc_ops = unsafe { &*ops };
    let result = unsafe {
        let path = args_ptr as *const RocStr;
        fs::create_dir_all((*path).as_str())
    };
    let try_result: TryUnitDirErr = match result {
        Ok(()) => RocTry::ok(()),
        Err(e) => {
            let io_err = roc_io_error::IOErr::from_io_error(&e, roc_ops);
            RocTry::err(RocSingleTagWrapper::new(io_err))
        }
    };
    unsafe {
        std::ptr::write(ret_ptr as *mut TryUnitDirErr, try_result);
    }
}

/// Hosted function: Dir.delete_all! (index 2)
/// Takes Str, returns Try({}, [DirErr(IOErr)])
extern "C" fn hosted_dir_delete_all(
    ops: *const RocOps,
    ret_ptr: *mut c_void,
    args_ptr: *mut c_void,
) {
    let roc_ops = unsafe { &*ops };
    let result = unsafe {
        let path = args_ptr as *const RocStr;
        fs::remove_dir_all((*path).as_str())
    };
    let try_result: TryUnitDirErr = match result {
        Ok(()) => RocTry::ok(()),
        Err(e) => {
            let io_err = roc_io_error::IOErr::from_io_error(&e, roc_ops);
            RocTry::err(RocSingleTagWrapper::new(io_err))
        }
    };
    unsafe {
        std::ptr::write(ret_ptr as *mut TryUnitDirErr, try_result);
    }
}

/// Hosted function: Dir.delete_empty! (index 3)
/// Takes Str, returns Try({}, [DirErr(IOErr)])
extern "C" fn hosted_dir_delete_empty(
    ops: *const RocOps,
    ret_ptr: *mut c_void,
    args_ptr: *mut c_void,
) {
    let roc_ops = unsafe { &*ops };
    let result = unsafe {
        let path = args_ptr as *const RocStr;
        fs::remove_dir((*path).as_str())
    };
    let try_result: TryUnitDirErr = match result {
        Ok(()) => RocTry::ok(()),
        Err(e) => {
            let io_err = roc_io_error::IOErr::from_io_error(&e, roc_ops);
            RocTry::err(RocSingleTagWrapper::new(io_err))
        }
    };
    unsafe {
        std::ptr::write(ret_ptr as *mut TryUnitDirErr, try_result);
    }
}

/// Hosted function: Dir.list! (index 4)
/// Takes Str, returns Try(List(Str), [DirErr(IOErr)])
extern "C" fn hosted_dir_list(
    ops: *const RocOps,
    ret_ptr: *mut c_void,
    args_ptr: *mut c_void,
) {
    let roc_ops = unsafe { &*ops };
    let path = unsafe {
        let args = args_ptr as *const RocStr;
        (*args).as_str().to_string()
    };

    let result = fs::read_dir(&path);
    let try_result: TryListStrDirErr = match result {
        Ok(rd) => {
            let entries: Vec<String> = rd
                .filter_map(|entry| {
                    entry.ok().map(|e| e.path().to_string_lossy().into_owned())
                })
                .collect();
            let mut list = RocList::with_capacity(entries.len(), roc_ops);
            for entry in entries {
                let roc_str = RocStr::from_str(&entry, roc_ops);
                list.push(roc_str, roc_ops);
            }
            RocTry::ok(list)
        }
        Err(e) => {
            let io_err = roc_io_error::IOErr::from_io_error(&e, roc_ops);
            RocTry::err(RocSingleTagWrapper::new(io_err))
        }
    };

    unsafe {
        std::ptr::write(ret_ptr as *mut TryListStrDirErr, try_result);
    }
}

/// Hosted function: Env.cwd! (index 5)
/// Takes {}, returns Str
extern "C" fn hosted_env_cwd(
    ops: *const RocOps,
    ret_ptr: *mut c_void,
    _args_ptr: *mut c_void,
) {
    let roc_ops = unsafe { &*ops };
    let cwd = std::env::current_dir()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default();
    let roc_str = RocStr::from_str(&cwd, roc_ops);
    unsafe {
        *(ret_ptr as *mut RocStr) = roc_str;
    }
}

/// Hosted function: Env.exe_path! (index 6)
/// Takes {}, returns Str
extern "C" fn hosted_env_exe_path(
    ops: *const RocOps,
    ret_ptr: *mut c_void,
    _args_ptr: *mut c_void,
) {
    let roc_ops = unsafe { &*ops };
    let exe_path = std::env::current_exe()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default();
    let roc_str = RocStr::from_str(&exe_path, roc_ops);
    unsafe {
        *(ret_ptr as *mut RocStr) = roc_str;
    }
}

/// Hosted function: Env.var! (index 7)
/// Takes Str, returns Str
extern "C" fn hosted_env_var(
    ops: *const RocOps,
    ret_ptr: *mut c_void,
    args_ptr: *mut c_void,
) {
    let roc_ops = unsafe { &*ops };
    let name = unsafe {
        let args = args_ptr as *const RocStr;
        (*args).as_str()
    };
    let value = std::env::var(name).unwrap_or_default();
    let roc_str = RocStr::from_str(&value, roc_ops);
    unsafe {
        *(ret_ptr as *mut RocStr) = roc_str;
    }
}

/// Hosted function: File.delete! (index 8)
/// Takes Str (path), returns Try({}, [FileErr(IOErr)])
extern "C" fn hosted_file_delete(
    ops: *const RocOps,
    ret_ptr: *mut c_void,
    args_ptr: *mut c_void,
) {
    let roc_ops = unsafe { &*ops };
    let path = unsafe {
        let args = args_ptr as *const RocStr;
        (*args).as_str()
    };
    let result = fs::remove_file(path);
    let try_result: TryUnitFileErr = match result {
        Ok(()) => RocTry::ok(()),
        Err(e) => {
            let io_err = roc_io_error::IOErr::from_io_error(&e, roc_ops);
            RocTry::err(RocSingleTagWrapper::new(io_err))
        }
    };
    unsafe {
        std::ptr::write(ret_ptr as *mut TryUnitFileErr, try_result);
    }
}

/// Hosted function: File.read_bytes! (index 9)
/// Takes Str (path), returns Try(List(U8), [FileErr(IOErr)])
extern "C" fn hosted_file_read_bytes(
    ops: *const RocOps,
    ret_ptr: *mut c_void,
    args_ptr: *mut c_void,
) {
    let roc_ops = unsafe { &*ops };
    let path = unsafe {
        let args = args_ptr as *const RocStr;
        (*args).as_str()
    };
    let result = fs::read(path);
    let try_result: TryBytesFileErr = match result {
        Ok(bytes) => {
            let mut list = RocList::with_capacity(bytes.len(), roc_ops);
            for byte in bytes {
                list.push(byte, roc_ops);
            }
            RocTry::ok(list)
        }
        Err(e) => {
            let io_err = roc_io_error::IOErr::from_io_error(&e, roc_ops);
            RocTry::err(RocSingleTagWrapper::new(io_err))
        }
    };
    unsafe {
        std::ptr::write(ret_ptr as *mut TryBytesFileErr, try_result);
    }
}

/// Hosted function: File.read_utf8! (index 10)
/// Takes Str (path), returns Try(Str, [FileErr(IOErr)])
extern "C" fn hosted_file_read_utf8(
    ops: *const RocOps,
    ret_ptr: *mut c_void,
    args_ptr: *mut c_void,
) {
    let roc_ops = unsafe { &*ops };
    let path = unsafe {
        let args = args_ptr as *const RocStr;
        (*args).as_str()
    };
    let result = fs::read_to_string(path);
    let try_result: TryStrFileErr = match result {
        Ok(content) => {
            let roc_str = RocStr::from_str(&content, roc_ops);
            RocTry::ok(roc_str)
        }
        Err(e) => {
            let io_err = roc_io_error::IOErr::from_io_error(&e, roc_ops);
            RocTry::err(RocSingleTagWrapper::new(io_err))
        }
    };
    unsafe {
        std::ptr::write(ret_ptr as *mut TryStrFileErr, try_result);
    }
}

/// Hosted function: File.write_bytes! (index 11)
/// Takes (Str, List(U8)), returns Try({}, [FileErr(IOErr)])
extern "C" fn hosted_file_write_bytes(
    ops: *const RocOps,
    ret_ptr: *mut c_void,
    args_ptr: *mut c_void,
) {
    let roc_ops = unsafe { &*ops };
    let result = unsafe {
        // Args are (Str, List(U8)) - a tuple/record
        let args = args_ptr as *const (RocStr, RocList<u8>);
        let (path, bytes) = &*args;
        fs::write(path.as_str(), bytes.as_slice())
    };
    let try_result: TryUnitFileErr = match result {
        Ok(()) => RocTry::ok(()),
        Err(e) => {
            let io_err = roc_io_error::IOErr::from_io_error(&e, roc_ops);
            RocTry::err(RocSingleTagWrapper::new(io_err))
        }
    };
    unsafe {
        std::ptr::write(ret_ptr as *mut TryUnitFileErr, try_result);
    }
}

/// Hosted function: File.write_utf8! (index 12)
/// Takes (Str, Str), returns Try({}, [FileErr(IOErr)])
extern "C" fn hosted_file_write_utf8(
    ops: *const RocOps,
    ret_ptr: *mut c_void,
    args_ptr: *mut c_void,
) {
    let roc_ops = unsafe { &*ops };
    let result = unsafe {
        // Args are (Str, Str) - a tuple
        let args = args_ptr as *const (RocStr, RocStr);
        let (path, content) = &*args;
        fs::write(path.as_str(), content.as_str())
    };
    let try_result: TryUnitFileErr = match result {
        Ok(()) => RocTry::ok(()),
        Err(e) => {
            let io_err = roc_io_error::IOErr::from_io_error(&e, roc_ops);
            RocTry::err(RocSingleTagWrapper::new(io_err))
        }
    };
    unsafe {
        std::ptr::write(ret_ptr as *mut TryUnitFileErr, try_result);
    }
}

/// Type alias for the Path error type: [PathErr(IOErr)] in Roc
type PathErr = RocSingleTagWrapper<roc_io_error::IOErr>;

/// Type alias for Try(Bool, [PathErr(IOErr)]) - used by Path.is_file!, etc.
type TryBoolPathErr = RocTry<bool, PathErr>;

/// Type alias for the File error type: [FileErr(IOErr)] in Roc
type FileErr = RocSingleTagWrapper<roc_io_error::IOErr>;

/// Type alias for Try({}, [FileErr(IOErr)]) - used by File.write_*, File.delete!
type TryUnitFileErr = RocTry<(), FileErr>;

/// Type alias for Try(Str, [FileErr(IOErr)]) - used by File.read_utf8!
type TryStrFileErr = RocTry<RocStr, FileErr>;

/// Type alias for Try(List(U8), [FileErr(IOErr)]) - used by File.read_bytes!
type TryBytesFileErr = RocTry<RocList<u8>, FileErr>;

/// Type alias for the Dir error type: [DirErr(IOErr)] in Roc
type DirErr = RocSingleTagWrapper<roc_io_error::IOErr>;

/// Type alias for Try({}, [DirErr(IOErr)]) - used by Dir.create!, etc.
type TryUnitDirErr = RocTry<(), DirErr>;

/// Type alias for Try(List(Str), [DirErr(IOErr)]) - used by Dir.list!
type TryListStrDirErr = RocTry<RocList<RocStr>, DirErr>;

/// Write a Try(Bool, [PathErr(IOErr)]) result to ret_ptr using RocTry
unsafe fn write_try_bool_result(
    ret_ptr: *mut c_void,
    result: std::io::Result<bool>,
    roc_ops: &RocOps,
) {
    let try_result: TryBoolPathErr = match result {
        Ok(value) => RocTry::ok(value),
        Err(e) => {
            let io_err = roc_io_error::IOErr::from_io_error(&e, roc_ops);
            RocTry::err(RocSingleTagWrapper::new(io_err))
        }
    };

    // Write the result to ret_ptr
    std::ptr::write(ret_ptr as *mut TryBoolPathErr, try_result);
}

/// Hosted function: Path.is_dir! (index 13)
/// Takes Str, returns Try(Bool, [PathErr(IOErr)])
extern "C" fn hosted_path_is_dir(
    ops: *const RocOps,
    ret_ptr: *mut c_void,
    args_ptr: *mut c_void,
) {
    let roc_ops = unsafe { &*ops };
    let result = unsafe {
        let path = args_ptr as *const RocStr;
        let path_str = (*path).as_str();
        std::path::Path::new(path_str)
            .symlink_metadata()
            .map(|m| m.is_dir())
    };

    unsafe {
        write_try_bool_result(ret_ptr, result, roc_ops);
    }
}

/// Hosted function: Path.is_file! (index 14)
/// Takes Str, returns Try(Bool, [PathErr(IOErr)])
extern "C" fn hosted_path_is_file(
    ops: *const RocOps,
    ret_ptr: *mut c_void,
    args_ptr: *mut c_void,
) {
    let roc_ops = unsafe { &*ops };
    let result = unsafe {
        let path = args_ptr as *const RocStr;
        let path_str = (*path).as_str();
        std::path::Path::new(path_str)
            .symlink_metadata()
            .map(|m| m.is_file())
    };

    unsafe {
        write_try_bool_result(ret_ptr, result, roc_ops);
    }
}

/// Hosted function: Path.is_sym_link! (index 15)
/// Takes Str, returns Try(Bool, [PathErr(IOErr)])
extern "C" fn hosted_path_is_sym_link(
    ops: *const RocOps,
    ret_ptr: *mut c_void,
    args_ptr: *mut c_void,
) {
    let roc_ops = unsafe { &*ops };
    let result = unsafe {
        let path = args_ptr as *const RocStr;
        let path_str = (*path).as_str();
        std::path::Path::new(path_str)
            .symlink_metadata()
            .map(|m| m.is_symlink())
    };

    unsafe {
        write_try_bool_result(ret_ptr, result, roc_ops);
    }
}

/// PathType discriminant values (alphabetically sorted)
/// IsDir=0, IsFile=1, IsSymLink=2
#[repr(u8)]
#[derive(Clone, Copy)]
pub enum PathType {
    IsDir = 0,
    IsFile = 1,
    IsSymLink = 2,
}

/// Type alias for Try(PathType, [PathErr(IOErr)]) - used by Path.type!
type TryPathTypePathErr = RocTry<PathType, PathErr>;

/// Write a Try(PathType, [PathErr(IOErr)]) result to ret_ptr using RocTry
unsafe fn write_try_pathtype_result(
    ret_ptr: *mut c_void,
    result: std::io::Result<PathType>,
    roc_ops: &RocOps,
) {
    let try_result: TryPathTypePathErr = match result {
        Ok(path_type) => RocTry::ok(path_type),
        Err(e) => {
            let io_err = roc_io_error::IOErr::from_io_error(&e, roc_ops);
            RocTry::err(RocSingleTagWrapper::new(io_err))
        }
    };

    std::ptr::write(ret_ptr as *mut TryPathTypePathErr, try_result);
}

/// Hosted function: Path.type! (index 16)
/// Takes Str, returns Try(PathType, [PathErr(IOErr)])
extern "C" fn hosted_path_type(
    ops: *const RocOps,
    ret_ptr: *mut c_void,
    args_ptr: *mut c_void,
) {
    let roc_ops = unsafe { &*ops };
    let result = unsafe {
        let path = args_ptr as *const RocStr;
        let path_str = (*path).as_str();
        std::path::Path::new(path_str).symlink_metadata().map(|m| {
            if m.is_symlink() {
                PathType::IsSymLink
            } else if m.is_dir() {
                PathType::IsDir
            } else {
                PathType::IsFile
            }
        })
    };

    unsafe {
        write_try_pathtype_result(ret_ptr, result, roc_ops);
    }
}

/// Hosted function: Stderr.line!
/// Takes Str, returns {}
extern "C" fn hosted_stderr_line(
    _ops: *const RocOps,
    _ret_ptr: *mut c_void,
    args_ptr: *mut c_void,
) {
    unsafe {
        // RocStr passed from Roc - Roc manages its memory, we just read it
        let args = args_ptr as *const RocStr;
        let message = (*args).as_str();
        let _ = writeln!(io::stderr(), "{}", message);
        // DO NOT call decref - Roc owns this memory
        // ret_ptr is for unit type {}, so we don't need to write anything
    }
}

/// Hosted function: Stderr.write! (index 17)
/// Takes Str, returns {}
extern "C" fn hosted_stderr_write(
    _ops: *const RocOps,
    _ret_ptr: *mut c_void,
    args_ptr: *mut c_void,
) {
    unsafe {
        // RocStr passed from Roc - Roc manages its memory, we just read it
        let args = args_ptr as *const RocStr;
        let message = (*args).as_str();
        let _ = write!(io::stderr(), "{}", message);
        let _ = io::stderr().flush();
        // DO NOT call decref - Roc owns this memory
    }
}

/// Hosted function: Stdin.line! (index 18)
/// Takes {}, returns Str
extern "C" fn hosted_stdin_line(
    ops: *const RocOps,
    ret_ptr: *mut c_void,
    _args_ptr: *mut c_void,
) {
    let mut line = String::new();
    let _ = io::stdin().lock().read_line(&mut line);

    // Create RocStr - ownership transfers to Roc
    let roc_ops = unsafe { &*ops };
    // Trim the trailing newline
    let roc_str = RocStr::from_str(line.trim_end_matches('\n'), roc_ops);

    unsafe {
        *(ret_ptr as *mut RocStr) = roc_str;
        // DO NOT call decref - ownership transferred to Roc
    }
}

/// Hosted function: Stdout.line! (index 19)
/// Takes Str, returns {}
extern "C" fn hosted_stdout_line(
    _ops: *const RocOps,
    _ret_ptr: *mut c_void,
    args_ptr: *mut c_void,
) {
    unsafe {
        // RocStr passed from Roc - Roc manages its memory, we just read it
        let args = args_ptr as *const RocStr;
        let message = (*args).as_str();
        let _ = writeln!(io::stdout(), "{}", message);
        // DO NOT call decref - Roc owns this memory
        // ret_ptr is for unit type {}, so we don't need to write anything
    }
}

/// Hosted function: Stdout.write! (index 20)
/// Takes Str, returns {}
extern "C" fn hosted_stdout_write(
    _ops: *const RocOps,
    _ret_ptr: *mut c_void,
    args_ptr: *mut c_void,
) {
    unsafe {
        // RocStr passed from Roc - Roc manages its memory, we just read it
        let args = args_ptr as *const RocStr;
        let message = (*args).as_str();
        let _ = write!(io::stdout(), "{}", message);
        let _ = io::stdout().flush();
        // DO NOT call decref - Roc owns this memory
    }
}

/// Array of hosted function pointers, sorted alphabetically by fully-qualified name.
/// IMPORTANT: Order must match the order Roc expects based on alphabetical sorting.
static HOSTED_FNS: [HostedFn; 22] = [
    hosted_dir_create,       // Dir.create!
    hosted_dir_create_all,   // Dir.create_all!
    hosted_dir_delete_all,   // Dir.delete_all!
    hosted_dir_delete_empty, // Dir.delete_empty!
    hosted_dir_list,         // Dir.list!
    hosted_env_cwd,          // Env.cwd!
    hosted_env_exe_path,     // Env.exe_path!
    hosted_env_var,          // Env.var!
    hosted_file_delete,      // File.delete!
    hosted_file_read_bytes,  // File.read_bytes!
    hosted_file_read_utf8,   // File.read_utf8!
    hosted_file_write_bytes, // File.write_bytes!
    hosted_file_write_utf8,  // File.write_utf8!
    hosted_path_is_dir,      // Path.is_dir!
    hosted_path_is_file,     // Path.is_file!
    hosted_path_is_sym_link, // Path.is_sym_link!
    hosted_path_type,        // Path.type!
    hosted_stderr_line,      // Stderr.line!
    hosted_stderr_write,     // Stderr.write!
    hosted_stdin_line,       // Stdin.line!
    hosted_stdout_line,      // Stdout.line!
    hosted_stdout_write,     // Stdout.write!
];

/// Build a RocList<RocStr> from command-line arguments.
fn build_args_list(roc_ops: &RocOps) -> RocList<RocStr> {
    let args: Vec<String> = std::env::args().collect();

    if args.is_empty() {
        return RocList::empty();
    }

    let mut list = RocList::with_capacity(args.len(), roc_ops);
    for arg in args {
        let roc_str = RocStr::from_str(&arg, roc_ops);
        list.push(roc_str, roc_ops);
    }
    list
}

/// C-compatible main entry point for the Roc program.
/// This is exported so the linker can find it.
#[no_mangle]
pub extern "C" fn main(_argc: i32, _argv: *const *const i8) -> i32 {
    rust_main()
}

/// Main entry point for the Roc program.
pub fn rust_main() -> i32 {
    // Create the RocOps struct with all callbacks
    // We Box it to ensure stable memory address
    let roc_ops = Box::new(RocOps {
        env: std::ptr::null_mut(),
        roc_alloc: roc_alloc_fn,
        roc_dealloc: roc_dealloc_fn,
        roc_realloc: roc_realloc_fn,
        roc_dbg: roc_dbg_fn,
        roc_expect_failed: roc_expect_failed_fn,
        roc_crashed: roc_crashed_fn,
        hosted_fns: HostedFunctions {
            count: HOSTED_FNS.len() as u32,
            fns: HOSTED_FNS.as_ptr(),
        },
    });

    // Build List(Str) from command-line arguments
    let args_list = build_args_list(&roc_ops);

    // Call the Roc main function
    let mut exit_code: i32 = -99;
    unsafe {
        roc__main_for_host(
            &*roc_ops,
            &mut exit_code as *mut i32 as *mut c_void,
            &args_list as *const RocList<RocStr> as *mut c_void,
        );
    }

    // If dbg or expect_failed was called, ensure non-zero exit code
    if DEBUG_OR_EXPECT_CALLED.load(Ordering::Acquire) && exit_code == 0 {
        return 1;
    }

    exit_code
}
