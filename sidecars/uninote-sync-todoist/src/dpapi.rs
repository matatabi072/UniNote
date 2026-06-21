//! Windows DPAPI による文字列暗号化/復号。
//! ユーザースコープのため、別ユーザー/別 PC では復号できない（流出時に安全）。
#![cfg(windows)]
use windows_sys::Win32::Security::Cryptography::{
    CryptProtectData, CryptUnprotectData, CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN,
};

// windows-sys 0.59 では LocalFree のエクスポート位置が版で揺れているため直接 extern 宣言する
extern "system" {
    fn LocalFree(hmem: *mut std::ffi::c_void) -> *mut std::ffi::c_void;
    fn GetLastError() -> u32;
}

unsafe fn make_blob(data: &[u8]) -> CRYPT_INTEGER_BLOB {
    CRYPT_INTEGER_BLOB {
        cbData: data.len() as u32,
        pbData: data.as_ptr() as *mut u8,
    }
}

pub fn encrypt(plain: &[u8]) -> Result<Vec<u8>, String> {
    unsafe {
        let mut input = make_blob(plain);
        let mut output = CRYPT_INTEGER_BLOB {
            cbData: 0,
            pbData: std::ptr::null_mut(),
        };
        let ok = CryptProtectData(
            &mut input,
            std::ptr::null(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut output,
        );
        if ok == 0 {
            let gle = GetLastError();
            return Err(format!("CryptProtectData 失敗 (GLE=0x{gle:08X})"));
        }
        let result = std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec();
        LocalFree(output.pbData as _);
        Ok(result)
    }
}

pub fn decrypt(cipher: &[u8]) -> Result<Vec<u8>, String> {
    unsafe {
        let mut input = make_blob(cipher);
        let mut output = CRYPT_INTEGER_BLOB {
            cbData: 0,
            pbData: std::ptr::null_mut(),
        };
        let ok = CryptUnprotectData(
            &mut input,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut output,
        );
        if ok == 0 {
            let gle = GetLastError();
            return Err(format!(
                "CryptUnprotectData 失敗 (GLE=0x{gle:08X}) — 別ユーザー/別PCで暗号化されたデータか、ファイル破損の可能性"
            ));
        }
        let result = std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec();
        LocalFree(output.pbData as _);
        Ok(result)
    }
}
