//! Minimal FFI to Quartz Display Services — enough to list online displays,
//! know which is built-in, and map IDs to the UUIDs macOS uses in the wallpaper plist.

use std::ffi::CStr;
use std::os::raw::{c_char, c_void};

type CGDirectDisplayID = u32;
type CGError = i32;
type Boolean = u8;
type CFIndex = isize;
type CFStringEncoding = u32;

#[repr(C)]
struct OpaqueRef {
    _p: [u8; 0],
}
type CFUUIDRef = *mut OpaqueRef;
type CFStringRef = *mut OpaqueRef;
type CFAllocatorRef = *mut OpaqueRef;

const K_CF_STRING_ENCODING_UTF8: CFStringEncoding = 0x0800_0100;

#[link(name = "CoreGraphics", kind = "framework")]
#[link(name = "CoreFoundation", kind = "framework")]
#[link(name = "ColorSync", kind = "framework")]
extern "C" {
    fn CGGetOnlineDisplayList(
        max_displays: u32,
        online_displays: *mut CGDirectDisplayID,
        display_count: *mut u32,
    ) -> CGError;
    fn CGDisplayIsBuiltin(display: CGDirectDisplayID) -> Boolean;
    // CGDisplayCreateUUIDFromDisplayID actually lives in ColorSync (documented in Quartz
    // Display Services but the symbol is there).
    fn CGDisplayCreateUUIDFromDisplayID(display: CGDirectDisplayID) -> CFUUIDRef;

    fn CFUUIDCreateString(alloc: CFAllocatorRef, uuid: CFUUIDRef) -> CFStringRef;
    fn CFStringGetLength(s: CFStringRef) -> CFIndex;
    fn CFStringGetCString(
        s: CFStringRef,
        buf: *mut c_char,
        buf_size: CFIndex,
        encoding: CFStringEncoding,
    ) -> Boolean;
    fn CFRelease(cf: *const c_void);
}

fn cfstring_to_string(s: CFStringRef) -> Option<String> {
    unsafe {
        let len = CFStringGetLength(s);
        // UTF-8 needs up to 4 bytes per UTF-16 unit, plus NUL.
        let cap = (len as usize) * 4 + 1;
        let mut buf = vec![0u8; cap];
        let ok = CFStringGetCString(
            s,
            buf.as_mut_ptr() as *mut c_char,
            cap as CFIndex,
            K_CF_STRING_ENCODING_UTF8,
        );
        if ok == 0 {
            return None;
        }
        Some(
            CStr::from_ptr(buf.as_ptr() as *const c_char)
                .to_string_lossy()
                .into_owned(),
        )
    }
}

fn display_uuid(id: CGDirectDisplayID) -> Option<String> {
    unsafe {
        let uuid = CGDisplayCreateUUIDFromDisplayID(id);
        if uuid.is_null() {
            return None;
        }
        let s = CFUUIDCreateString(std::ptr::null_mut(), uuid);
        CFRelease(uuid as *const c_void);
        if s.is_null() {
            return None;
        }
        let out = cfstring_to_string(s);
        CFRelease(s as *const c_void);
        out
    }
}

/// Returns (display_id, is_builtin, plist_uuid) for each online display.
/// If UUID lookup fails for a display it's skipped.
pub fn online_displays() -> Vec<(CGDirectDisplayID, bool, String)> {
    const MAX: usize = 32;
    let mut ids = [0u32; MAX];
    let mut count: u32 = 0;
    let err = unsafe { CGGetOnlineDisplayList(MAX as u32, ids.as_mut_ptr(), &mut count) };
    if err != 0 {
        return Vec::new();
    }
    let mut out = Vec::with_capacity(count as usize);
    for &id in &ids[..count as usize] {
        let is_builtin = unsafe { CGDisplayIsBuiltin(id) } != 0;
        if let Some(uuid) = display_uuid(id) {
            out.push((id, is_builtin, uuid));
        }
    }
    out
}
