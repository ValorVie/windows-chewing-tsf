// SPDX-License-Identifier: GPL-3.0-or-later

use chewing_tip_core::sandbox::get_user_cred;
use windows::Win32::System::Threading::{GetCurrentProcess, GetProcessHandleCount};

fn handle_count() -> u32 {
    let mut count = 0;
    unsafe {
        GetProcessHandleCount(GetCurrentProcess(), &mut count).unwrap();
    }
    count
}

#[test]
fn repeated_ipc_identity_queries_release_their_token_handles() {
    let expected = get_user_cred().unwrap().token_user_sid;
    let before = handle_count();
    for _ in 0..512 {
        assert!(
            get_user_cred().unwrap().token_user_sid == expected,
            "identity changed"
        );
    }
    let after = handle_count();
    println!("C2_TOKEN_HANDLES_BEFORE={before},AFTER={after}");
    assert!(
        after <= before + 4,
        "token handle growth: {before} -> {after}"
    );
}
