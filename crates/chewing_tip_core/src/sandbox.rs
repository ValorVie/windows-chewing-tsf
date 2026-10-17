use std::mem::transmute;

use scoped_error::{expect_error, impl_context_error};
use windows::{
    Win32::{
        Foundation::{
            CloseHandle, ERROR_INSUFFICIENT_BUFFER, ERROR_NO_TOKEN, HANDLE, HLOCAL, LocalFree,
        },
        Security::{
            Authorization::ConvertSidToStringSidW, GetTokenInformation, TOKEN_ACCESS_MASK,
            TOKEN_QUERY, TOKEN_USER, TokenUser,
        },
        System::Threading::{
            GetCurrentProcess, GetCurrentThread, OpenProcessToken, OpenThreadToken,
        },
    },
    core::PWSTR,
};

/// Represents the user credential including SID
pub struct UserCred {
    pub token_user_sid: String,
}

pub fn open_effective_token(desired_access: TOKEN_ACCESS_MASK) -> Result<HANDLE, SandboxError> {
    expect_error("Unable to get effective token", || unsafe {
        let mut token = HANDLE::default();
        if let Err(error) = OpenThreadToken(GetCurrentThread(), desired_access, true, &mut token) {
            if error.code() != ERROR_NO_TOKEN.into() {
                Err(error)?;
            }
            if let Err(error) = OpenProcessToken(GetCurrentProcess(), desired_access, &mut token) {
                Err(error)?;
            }
        }
        Ok(token)
    })
}

pub fn get_token_user_sid_string(token: HANDLE) -> Result<String, SandboxError> {
    expect_error("Failed to extract user SID from token", || {
        let mut buffer_size = 0;
        unsafe {
            if let Err(error) = GetTokenInformation(token, TokenUser, None, 0, &mut buffer_size) {
                if error != ERROR_INSUFFICIENT_BUFFER.into() {
                    Err(error)?;
                }
            }
        }
        let mut _return_size = 0;
        let mut buffer = vec![0; buffer_size as usize];
        unsafe {
            GetTokenInformation(
                token,
                TokenUser,
                Some(buffer.as_mut_ptr().cast()),
                buffer_size,
                &mut _return_size,
            )?;
        }
        unsafe {
            let token_user: *const TOKEN_USER = transmute(buffer.as_ptr());
            let sid = token_user.as_ref().unwrap().User.Sid;
            let mut sid_string = PWSTR::null();
            ConvertSidToStringSidW(sid, &mut sid_string)?;
            let result = sid_string.to_string();
            // ConvertSidToStringSidW allocates this buffer with LocalAlloc.
            // Release it even when UTF-16 conversion fails.
            let freed = LocalFree(Some(HLOCAL(sid_string.0.cast())));
            if !freed.0.is_null() {
                return Err("failed to free SID buffer".into());
            }
            Ok(result?)
        }
    })
}

pub fn get_user_cred() -> Result<UserCred, SandboxError> {
    expect_error("Failed to get user credential", || {
        let token = open_effective_token(TOKEN_QUERY)?;
        let result = get_token_user_sid_string(token);
        let closed = unsafe { CloseHandle(token) };
        let token_user_sid = result?;
        closed?;
        Ok(UserCred { token_user_sid })
    })
}

impl_context_error!(pub SandboxError);
