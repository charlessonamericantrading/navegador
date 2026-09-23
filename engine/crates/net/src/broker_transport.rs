//! El canal local entre renderer y broker (ADR 0001, etapa 2).
//!
//! En Windows, una tubería con nombre; en el resto, un socket Unix. Los dos
//! son locales por construcción (no se puede llegar a ellos por red) y los
//! dos dan un flujo de bytes bidireccional sobre el que va
//! `crate::broker_wire`.
//!
//! El nombre del canal es aleatorio, pero **no es un secreto**: lo que
//! autentica a un renderer es el token que presenta al conectar (ver
//! `crate::broker_server`). Aun así se hace difícil de adivinar y, en
//! Windows, se crea como primera instancia para que otro proceso no pueda
//! adelantarse a crear una tubería con el mismo nombre y hacerse pasar por
//! el broker.

use std::io;
use tokio::io::{AsyncRead, AsyncWrite};

/// Flujo bidireccional, del tipo que sea en cada sistema.
pub trait Duplex: AsyncRead + AsyncWrite + Unpin + Send + 'static {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send + 'static> Duplex for T {}

pub type Stream = Box<dyn Duplex>;

/// `bytes` bytes aleatorios del sistema, en hexadecimal.
pub fn random_hex(bytes: usize) -> io::Result<String> {
    let mut buffer = vec![0u8; bytes];
    getrandom::getrandom(&mut buffer).map_err(|e| io::Error::other(format!("sin aleatoriedad del sistema: {e}")))?;
    Ok(buffer.iter().map(|b| format!("{b:02x}")).collect())
}

/// Comparación que tarda lo mismo acierte o falle en el primer carácter:
/// comparar tokens con `==` deja medir cuántos caracteres iniciales son
/// correctos.
pub fn constant_time_eq(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

#[cfg(windows)]
mod platform {
    use super::*;
    use tokio::net::windows::named_pipe::{ClientOptions, NamedPipeServer, ServerOptions};

    /// `ERROR_PIPE_BUSY`: todas las instancias ocupadas; se reintenta.
    const ERROR_PIPE_BUSY: i32 = 231;

    pub struct Listener {
        name: String,
        next: NamedPipeServer,
    }

    impl Listener {
        pub fn bind() -> io::Result<Self> {
            let name = format!(r"\\.\pipe\navegador-ia-broker-{}-{}", std::process::id(), random_hex(16)?);
            let next = ServerOptions::new().first_pipe_instance(true).reject_remote_clients(true).create(&name)?;
            Ok(Self { name, next })
        }

        pub fn endpoint(&self) -> &str {
            &self.name
        }

        /// Espera al siguiente cliente. Cada instancia de tubería sirve a un
        /// solo cliente, así que antes de entregar la conectada se crea la
        /// siguiente: nunca hay un momento sin instancia esperando.
        pub async fn accept(&mut self) -> io::Result<Stream> {
            self.next.connect().await?;
            let fresh = ServerOptions::new().reject_remote_clients(true).create(&self.name)?;
            Ok(Box::new(std::mem::replace(&mut self.next, fresh)))
        }
    }

    pub async fn connect(endpoint: &str) -> io::Result<Stream> {
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            match ClientOptions::new().open(endpoint) {
                Ok(client) => return Ok(Box::new(client)),
                Err(e) if e.raw_os_error() == Some(ERROR_PIPE_BUSY) && tokio::time::Instant::now() < deadline => {
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                }
                Err(e) => return Err(e),
            }
        }
    }
}

#[cfg(unix)]
mod platform {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;
    use tokio::net::{UnixListener, UnixStream};

    pub struct Listener {
        path: PathBuf,
        endpoint: String,
        listener: UnixListener,
    }

    impl Listener {
        pub fn bind() -> io::Result<Self> {
            // Nombre corto a propósito: la ruta de un socket Unix tiene un
            // límite de ~104-108 bytes, y el directorio temporal de macOS ya
            // se come la mitad.
            let path = std::env::temp_dir().join(format!("nia-broker-{}.sock", random_hex(8)?));
            let listener = UnixListener::bind(&path)?;
            // Solo el usuario: nadie más puede ni intentar el saludo.
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
            let endpoint = path.to_string_lossy().into_owned();
            Ok(Self { path, endpoint, listener })
        }

        pub fn endpoint(&self) -> &str {
            &self.endpoint
        }

        pub async fn accept(&mut self) -> io::Result<Stream> {
            let (stream, _) = self.listener.accept().await?;
            Ok(Box::new(stream))
        }
    }

    impl Drop for Listener {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.path);
        }
    }

    pub async fn connect(endpoint: &str) -> io::Result<Stream> {
        Ok(Box::new(UnixStream::connect(endpoint).await?))
    }
}

pub use platform::{connect, Listener};

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[test]
    fn tokens_compare_by_content() {
        assert!(constant_time_eq("abc", "abc"));
        assert!(!constant_time_eq("abc", "abd"));
        assert!(!constant_time_eq("abc", "abcd"));
    }

    #[test]
    fn random_hex_has_the_requested_length_and_varies() {
        let a = random_hex(32).unwrap();
        assert_eq!(a.len(), 64);
        assert_ne!(a, random_hex(32).unwrap());
    }

    /// Dos clientes seguidos contra el mismo canal: el segundo también
    /// encuentra una instancia esperando.
    #[tokio::test]
    async fn the_listener_serves_several_clients() {
        let mut listener = Listener::bind().unwrap();
        let endpoint = listener.endpoint().to_string();
        let servidor = tokio::spawn(async move {
            for _ in 0..2 {
                let mut stream = listener.accept().await.unwrap();
                let mut byte = [0u8; 1];
                stream.read_exact(&mut byte).await.unwrap();
                stream.write_all(&[byte[0] + 1]).await.unwrap();
            }
        });
        for valor in [1u8, 10] {
            let mut cliente = connect(&endpoint).await.unwrap();
            cliente.write_all(&[valor]).await.unwrap();
            let mut respuesta = [0u8; 1];
            cliente.read_exact(&mut respuesta).await.unwrap();
            assert_eq!(respuesta[0], valor + 1);
        }
        servidor.await.unwrap();
    }
}
