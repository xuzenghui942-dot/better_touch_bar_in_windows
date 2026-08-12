//! Minimal Rust/COM bridge for Windows' internal virtual-desktop manager.
//!
//! The public `IVirtualDesktopManager` refuses to move windows owned by another
//! process. Swoosh uses the shell's internal application-view interface for
//! precisely this reason; this module mirrors that call sequence and falls
//! back cleanly when a Windows build changes the undocumented vtable.

use std::{ffi::c_void, mem};

use windows_sys::{
    core::GUID,
    Win32::{
        Foundation::HWND,
        System::Com::{
            CoCreateInstance, CoInitializeEx, CLSCTX_LOCAL_SERVER, COINIT_APARTMENTTHREADED,
        },
    },
};

type Raw = *mut c_void;

const CLSID_IMMERSIVE_SHELL: GUID = GUID::from_u128(0xc2f03a33_21f5_47fa_b4bb_156362a2f239);
const CLSID_VDM_INTERNAL: GUID = GUID::from_u128(0xc5e0cdca_7b6e_41b2_9fc4_d93975cc467b);
const IID_SERVICE_PROVIDER: GUID = GUID::from_u128(0x6d5140c1_7436_11ce_8034_00aa006009fa);
const IID_VIEW_COLLECTION: GUID = GUID::from_u128(0x1841c6d7_4f9d_42c0_af41_8747538f10e5);
const IID_VDM_INTERNAL: GUID = GUID::from_u128(0x53f5ca0b_158f_4124_900c_057158060b27);

pub fn move_by_steps(hwnd: HWND, direction: i32, steps: i32) -> bool {
    if hwnd.is_null() || steps <= 0 {
        return false;
    }
    unsafe {
        // The worker thread owns the input message loop, so initialize COM once
        // for this call. S_FALSE and RPC_E_CHANGED_MODE are both safe to ignore.
        let _ = CoInitializeEx(std::ptr::null(), COINIT_APARTMENTTHREADED as u32);
        let mut shell: Raw = std::ptr::null_mut();
        if CoCreateInstance(
            &CLSID_IMMERSIVE_SHELL,
            std::ptr::null_mut(),
            CLSCTX_LOCAL_SERVER,
            &IID_SERVICE_PROVIDER,
            &mut shell,
        ) < 0
        {
            return false;
        }
        let mut internal: Raw = std::ptr::null_mut();
        let mut views: Raw = std::ptr::null_mut();
        let service =
            vtable_call_query_service(shell, &CLSID_VDM_INTERNAL, &IID_VDM_INTERNAL, &mut internal);
        let views_hr = vtable_call_query_service(
            shell,
            &IID_VIEW_COLLECTION,
            &IID_VIEW_COLLECTION,
            &mut views,
        );
        release(shell);
        if service < 0 || views_hr < 0 || internal.is_null() || views.is_null() {
            release(internal);
            release(views);
            return false;
        }

        let mut view: Raw = std::ptr::null_mut();
        let get_view = vtable_fn::<unsafe extern "system" fn(Raw, HWND, *mut Raw) -> i32>(views, 6);
        if get_view(views, hwnd, &mut view) < 0 || view.is_null() {
            release(internal);
            release(views);
            return false;
        }
        let get_current = vtable_fn::<unsafe extern "system" fn(Raw) -> Raw>(internal, 6);
        let current = get_current(internal);
        if current.is_null() {
            release(view);
            release(internal);
            release(views);
            return false;
        }

        let get_adjacent =
            vtable_fn::<unsafe extern "system" fn(Raw, Raw, i32, *mut Raw) -> i32>(internal, 8);
        let move_view = vtable_fn::<unsafe extern "system" fn(Raw, Raw, Raw)>(internal, 4);
        let switch = vtable_fn::<unsafe extern "system" fn(Raw, Raw)>(internal, 22);
        let mut cursor = current;
        let mut target = std::ptr::null_mut();
        for _ in 0..steps.min(8) {
            let mut next = std::ptr::null_mut();
            if get_adjacent(internal, cursor, direction, &mut next) < 0 || next.is_null() {
                break;
            }
            if cursor != current {
                release(cursor);
            }
            cursor = next;
            target = next;
        }
        if target.is_null() {
            release(current);
            release(view);
            release(internal);
            release(views);
            return false;
        }
        move_view(internal, view, target);
        switch(internal, target);
        if cursor != current {
            release(cursor);
        }
        release(current);
        release(view);
        release(internal);
        release(views);
        true
    }
}

/// Create a desktop at the right edge and move the target view to it.  This is
/// the source application's "create desktop on overflow" path.  The shell
/// interfaces are undocumented, so a failure is reported to the caller and it
/// can retain the ordinary Win+Ctrl+Arrow fallback instead of crashing.
pub fn move_to_new_desktop(hwnd: HWND, follow_hwnd: HWND) -> bool {
    if hwnd.is_null() {
        return false;
    }
    unsafe {
        let _ = CoInitializeEx(std::ptr::null(), COINIT_APARTMENTTHREADED as u32);
        let Some((internal, views)) = acquire_services() else {
            return false;
        };

        let mut view: Raw = std::ptr::null_mut();
        let get_view = vtable_fn::<unsafe extern "system" fn(Raw, HWND, *mut Raw) -> i32>(views, 6);
        if get_view(views, hwnd, &mut view) < 0 || view.is_null() {
            release(internal);
            release(views);
            return false;
        }

        // IVirtualDesktopManagerInternal::CreateDesktop is method 8 after the
        // IUnknown trio, i.e. raw vtable index 11.
        let create = vtable_fn::<unsafe extern "system" fn(Raw) -> Raw>(internal, 11);
        let target = create(internal);
        if target.is_null() {
            release(view);
            release(internal);
            release(views);
            return false;
        }

        let move_view = vtable_fn::<unsafe extern "system" fn(Raw, Raw, Raw)>(internal, 4);
        move_view(internal, view, target);

        // Carry the preview window when the shell exposes it as an application
        // view.  The overlay is best-effort and must never affect the target
        // move result.
        if !follow_hwnd.is_null() {
            let mut follow_view: Raw = std::ptr::null_mut();
            if get_view(views, follow_hwnd, &mut follow_view) >= 0 && !follow_view.is_null() {
                move_view(internal, follow_view, target);
                release(follow_view);
            }
        }

        let switch = vtable_fn::<unsafe extern "system" fn(Raw, Raw)>(internal, 22);
        switch(internal, target);
        release(view);
        release(target);
        release(internal);
        release(views);
        true
    }
}

/// Return `(desktop_count, current_index)` using the same adjacent-desktop
/// walk as Swoosh's HUD.  This is intentionally best-effort because Explorer
/// can recycle the internal COM server while a desktop is being changed.
pub fn layout() -> Option<(i32, i32)> {
    unsafe {
        let _ = CoInitializeEx(std::ptr::null(), COINIT_APARTMENTTHREADED as u32);
        let (internal, views) = acquire_services()?;
        let get_count = vtable_fn::<unsafe extern "system" fn(Raw) -> i32>(internal, 3);
        let count = get_count(internal);
        if count <= 0 {
            release(internal);
            release(views);
            return None;
        }
        let get_current = vtable_fn::<unsafe extern "system" fn(Raw) -> Raw>(internal, 6);
        let current = get_current(internal);
        if current.is_null() {
            release(internal);
            release(views);
            return None;
        }
        let get_adjacent =
            vtable_fn::<unsafe extern "system" fn(Raw, Raw, i32, *mut Raw) -> i32>(internal, 8);
        let mut cursor = current;
        let mut index = 0_i32;
        while index < count {
            let mut previous = std::ptr::null_mut();
            if get_adjacent(internal, cursor, 3, &mut previous) < 0 || previous.is_null() {
                break;
            }
            if cursor != current {
                release(cursor);
            }
            cursor = previous;
            index += 1;
        }
        if cursor != current {
            release(cursor);
        }
        release(current);
        release(internal);
        release(views);
        Some((count, index))
    }
}

unsafe fn acquire_services() -> Option<(Raw, Raw)> {
    let mut shell: Raw = std::ptr::null_mut();
    if CoCreateInstance(
        &CLSID_IMMERSIVE_SHELL,
        std::ptr::null_mut(),
        CLSCTX_LOCAL_SERVER,
        &IID_SERVICE_PROVIDER,
        &mut shell,
    ) < 0
    {
        return None;
    }
    let mut internal: Raw = std::ptr::null_mut();
    let mut views: Raw = std::ptr::null_mut();
    let service =
        vtable_call_query_service(shell, &CLSID_VDM_INTERNAL, &IID_VDM_INTERNAL, &mut internal);
    let views_hr = vtable_call_query_service(
        shell,
        &IID_VIEW_COLLECTION,
        &IID_VIEW_COLLECTION,
        &mut views,
    );
    release(shell);
    if service < 0 || views_hr < 0 || internal.is_null() || views.is_null() {
        release(internal);
        release(views);
        return None;
    }
    Some((internal, views))
}

unsafe fn vtable_call_query_service(
    object: Raw,
    service: *const GUID,
    iid: *const GUID,
    result: *mut Raw,
) -> i32 {
    let query = vtable_fn::<
        unsafe extern "system" fn(Raw, *const GUID, *const GUID, *mut Raw) -> i32,
    >(object, 3);
    query(object, service, iid, result)
}

unsafe fn vtable_fn<T: Copy>(object: Raw, index: usize) -> T {
    let table = *(object as *const *const *const c_void);
    mem::transmute_copy(&*table.add(index))
}

unsafe fn release(object: Raw) {
    if !object.is_null() {
        let release = vtable_fn::<unsafe extern "system" fn(Raw) -> u32>(object, 2);
        release(object);
    }
}
