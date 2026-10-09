use anyhow::{ensure, Context, Result};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

pub const MODEL_REVISION: &str = "751bff37182d3f1213fa05d7196b954e230abad9";
const FILES: [(&str, &str); 3] = [
    (
        "tokenizer.json",
        "da0e79933b9ed51798a3ae27893d3c5fa4a201126cef75586296df9b4d2c62a0",
    ),
    (
        "config.json",
        "7135149f7cffa1a573466c6e4d8423ed73b62fd2332c575bf738a0d033f70df7",
    ),
    (
        "onnx/model_quantized.onnx",
        "afdb6f1a0e45b715d0bb9b11772f032c399babd23bfc31fed1c170afc848bdb1",
    ),
];
fn verify(bytes: &[u8], expected: &str, name: &str) -> Result<()> {
    ensure!(
        Sha256::digest(bytes)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
            == expected,
        "model SHA-256 mismatch: {name}; remove the damaged cache file and run why fetch-model"
    );
    Ok(())
}
/// Verify every model input before loading it, including previously cached files.
pub fn verify_model(dir: &Path) -> Result<()> {
    for (file, digest) in FILES {
        verify(
            &std::fs::read(dir.join(file)).with_context(|| format!("read model {file}"))?,
            digest,
            file,
        )?;
    }
    Ok(())
}
/// Download immutable, digest-verified model inputs. Existing files must verify.
pub fn fetch_model() -> Result<PathBuf> {
    ensure!(
        cfg!(feature = "local-embeddings"),
        "fetch-model requires the local-embeddings Cargo feature"
    );
    let dir = super::model_cache_dir();
    for (file, digest) in FILES {
        let dest = dir.join(file);
        if dest.exists() {
            verify(&std::fs::read(&dest)?, digest, file)?;
            continue;
        }
        std::fs::create_dir_all(dest.parent().context("model parent")?)?;
        let url = format!(
            "https://huggingface.co/Xenova/all-MiniLM-L6-v2/resolve/{MODEL_REVISION}/{file}"
        );
        let bytes = ureq::get(&url)
            .call()
            .with_context(|| format!("download {url}"))?
            .body_mut()
            .with_config()
            .limit(100 * 1024 * 1024)
            .read_to_vec()?;
        verify(&bytes, digest, file)?;
        // Create-exclusive staging plus atomic rename prevents partially downloaded inputs.
        let tmp = dest.with_extension(format!("download-{}", std::process::id()));
        use std::io::Write;
        let mut output = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)?;
        let result = (|| -> Result<()> {
            output.write_all(&bytes)?;
            output.sync_all()?;
            std::fs::rename(&tmp, &dest)?;
            Ok(())
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
        result?;
    }
    verify_model(&dir)?;
    Ok(dir)
}
#[cfg(test)]
mod tests {
    #[test]
    fn damaged_model_input_is_rejected() {
        assert!(super::verify(b"damaged", super::FILES[0].1, "tokenizer.json").is_err());
        assert!(super::verify(
            b"abc",
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
            "test"
        )
        .is_ok());
    }
}
