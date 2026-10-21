//! Non-actuating stock-image experiment: no SDK, globals, MMIO or flash writes.
//!
//! `tools/prepare-c-probe.py` links this archive with `firmware/probe/probe.ld`
//! at the reviewed padding slot and patches GET_CONFIG's `softwareVersion` add
//! call to `probe_add_version`, which adds a `localReProbe` proof field.
//! Host builds are empty: the stock cJSON routines exist only in the image,
//! and behavior is verified by `tools/emulate-c-probe.py` on real ARM code.
#![cfg_attr(target_os = "none", no_std)]

#[cfg(target_os = "none")]
mod probe {
    use core::ptr;

    /// Reviewed 32-bit stock cJSON layout; no persistent probe state.
    #[repr(C)]
    pub struct ProbeJson {
        pub next: *mut ProbeJson,
        pub previous: *mut ProbeJson,
        pub child: *mut ProbeJson,
        pub kind: i32,
        pub string_value: *mut u8,
        pub integer_value: i32,
        pub double_value_words: [u32; 2],
        pub key: *mut u8,
    }

    const _: () = assert!(
        size_of::<*const u8>() == 4,
        "This hook requires the reviewed 32-bit ABI"
    );
    const _: () = assert!(
        core::mem::offset_of!(ProbeJson, string_value) == 16,
        "Stock cJSON valuestring offset changed"
    );
    const _: () = assert!(
        core::mem::offset_of!(ProbeJson, key) == 32,
        "Stock cJSON key offset changed"
    );

    // Stock 1.1.15 routines; probe.ld pins their reviewed addresses.
    unsafe extern "C" {
        fn stock_json_add(object: *mut ProbeJson, key: *const u8, value: *mut ProbeJson);
        fn stock_json_find(object: *mut ProbeJson, key: *const u8) -> *mut ProbeJson;
        fn stock_json_string(value: *const u8) -> *mut ProbeJson;
        fn stock_json_duplicate(value: *const u8) -> *mut u8;
        fn stock_json_append(object: *mut ProbeJson, value: *mut ProbeJson);
        fn stock_json_delete(value: *mut ProbeJson);
    }

    const DIGITS: &[u8; 16] = b"0123456789abcdef";

    /// Replaces only GET_CONFIG's softwareVersion add call, then adds a proof.
    ///
    /// # Safety
    /// Called by the patched stock handler with its cJSON arguments.
    #[unsafe(no_mangle)]
    #[unsafe(link_section = ".text.probe_add_version")]
    pub unsafe extern "C" fn probe_add_version(
        object: *mut ProbeJson,
        key: *const u8,
        value: *mut ProbeJson,
    ) {
        unsafe {
            // Execute the displaced stock operation exactly once, including on failure.
            stock_json_add(object, key, value);
            if object.is_null() {
                return;
            }
            let message = stock_json_find(object, c"msgId".as_ptr().cast());
            if message.is_null() || (*message).kind & 255 != 4 || (*message).string_value.is_null()
            {
                return;
            }

            // The stock generator uses a 48-byte message-ID buffer. Do not read
            // past it. FNV-1a is an execution diagnostic, not authentication.
            let text = (*message).string_value;
            let mut hash: u32 = 2_166_136_261;
            let mut length = 0;
            while length < 48 {
                let byte = ptr::read(text.add(length));
                if byte == 0 {
                    break;
                }
                hash ^= u32::from(byte);
                hash = hash.wrapping_mul(16_777_619);
                length += 1;
            }
            if length == 0 || length == 48 {
                return;
            }

            let mut proof = *b"c1-00000000\0";
            // Constant offsets keep every stack store a reviewed immediate form
            // (the emulator refuses register-indexed stores) with no panic path.
            let p = proof.as_mut_ptr();
            let d = DIGITS.as_ptr();
            *p.add(10) = *d.add((hash & 15) as usize);
            hash >>= 4;
            *p.add(9) = *d.add((hash & 15) as usize);
            hash >>= 4;
            *p.add(8) = *d.add((hash & 15) as usize);
            hash >>= 4;
            *p.add(7) = *d.add((hash & 15) as usize);
            hash >>= 4;
            *p.add(6) = *d.add((hash & 15) as usize);
            hash >>= 4;
            *p.add(5) = *d.add((hash & 15) as usize);
            hash >>= 4;
            *p.add(4) = *d.add((hash & 15) as usize);
            hash >>= 4;
            *p.add(3) = *d.add((hash & 15) as usize);
            // The reviewed stock constructor copies this stack string. The
            // normal GET_CONFIG destructor owns and frees the added node/key/value.
            let diagnostic = stock_json_string(proof.as_ptr());
            if diagnostic.is_null() {
                return;
            }
            if (*diagnostic).string_value.is_null() {
                stock_json_delete(diagnostic);
                return;
            }
            let diagnostic_key = stock_json_duplicate(c"localReProbe".as_ptr().cast());
            if diagnostic_key.is_null() {
                stock_json_delete(diagnostic);
                return;
            }
            // The stock AddItemToObject links a node even if its key allocation
            // fails. Prepare both strings before publishing ownership.
            (*diagnostic).key = diagnostic_key;
            stock_json_append(object, diagnostic);
        }
    }

    #[panic_handler]
    fn panic(_: &core::panic::PanicInfo<'_>) -> ! {
        loop {}
    }
}
