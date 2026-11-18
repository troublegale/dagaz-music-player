use crate::ClientError;
use std::path::Path;

#[cfg(windows)]
mod native {
    use super::*;
    use sha2::{Digest, Sha256};
    use windows_sys::Win32::{
        Foundation::{CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HANDLE, LocalFree},
        Security::Cryptography::{
            CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptProtectData, CryptUnprotectData,
        },
        System::Threading::CreateMutexW,
    };
    pub struct InstanceGuard(HANDLE);
    impl InstanceGuard {
        pub fn acquire(profile: &Path) -> Result<Self, ClientError> {
            std::fs::create_dir_all(profile).map_err(|_| ClientError::Storage)?;
            let path = profile.canonicalize().map_err(|_| ClientError::Storage)?;
            let digest = format!(
                "{:x}",
                Sha256::digest(path.to_string_lossy().to_lowercase().as_bytes())
            );
            let name: Vec<u16> = format!("Global\\Dagaz.{digest}\0").encode_utf16().collect();
            // Existence, rather than mutex ownership, fences the profile for the process lifetime.
            unsafe {
                let handle = CreateMutexW(std::ptr::null(), 0, name.as_ptr());
                if handle.is_null() {
                    return Err(ClientError::Storage);
                }
                if GetLastError() == ERROR_ALREADY_EXISTS {
                    CloseHandle(handle);
                    return Err(ClientError::AlreadyRunning);
                }
                Ok(Self(handle))
            }
        }
    }
    impl Drop for InstanceGuard {
        fn drop(&mut self) {
            unsafe {
                CloseHandle(self.0);
            }
        }
    }
    pub fn crypt(bytes: &[u8], encrypt: bool) -> Result<Vec<u8>, ClientError> {
        let input = CRYPT_INTEGER_BLOB {
            cbData: bytes.len().try_into().map_err(|_| ClientError::Storage)?,
            pbData: bytes.as_ptr().cast_mut(),
        };
        let mut output = CRYPT_INTEGER_BLOB {
            cbData: 0,
            pbData: std::ptr::null_mut(),
        };
        // DPAPI owns output; copy it before LocalFree and forbid all interactive credential prompts.
        unsafe {
            let success = if encrypt {
                CryptProtectData(
                    &input,
                    std::ptr::null(),
                    std::ptr::null(),
                    std::ptr::null(),
                    std::ptr::null(),
                    CRYPTPROTECT_UI_FORBIDDEN,
                    &mut output,
                )
            } else {
                CryptUnprotectData(
                    &input,
                    std::ptr::null_mut(),
                    std::ptr::null(),
                    std::ptr::null(),
                    std::ptr::null(),
                    CRYPTPROTECT_UI_FORBIDDEN,
                    &mut output,
                )
            };
            if success == 0 {
                return Err(if encrypt {
                    ClientError::Storage
                } else {
                    ClientError::PairingRequired
                });
            }
            let result = std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec();
            LocalFree(output.pbData.cast());
            Ok(result)
        }
    }
}
#[cfg(windows)]
pub use native::InstanceGuard;
#[cfg(windows)]
pub(crate) fn protect(bytes: &[u8]) -> Result<Vec<u8>, ClientError> {
    native::crypt(bytes, true)
}
#[cfg(windows)]
pub(crate) fn unprotect(bytes: &[u8]) -> Result<Vec<u8>, ClientError> {
    native::crypt(bytes, false)
}

#[cfg(not(windows))]
pub struct InstanceGuard;
#[cfg(not(windows))]
impl InstanceGuard {
    pub fn acquire(_: &Path) -> Result<Self, ClientError> {
        Err(ClientError::UnsupportedPlatform)
    }
}
#[cfg(not(windows))]
pub(crate) fn protect(_: &[u8]) -> Result<Vec<u8>, ClientError> {
    Err(ClientError::UnsupportedPlatform)
}
#[cfg(not(windows))]
pub(crate) fn unprotect(_: &[u8]) -> Result<Vec<u8>, ClientError> {
    Err(ClientError::UnsupportedPlatform)
}
