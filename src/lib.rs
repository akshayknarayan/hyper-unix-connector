//! Connect hyper servers and clients to Unix-domain sockets.
//!
//! Most of this crate's functionality is borrowed from [hyperlocal](https://github.com/softprops/hyperlocal).
//! This crate supports async/await, while hyperlocal does not (yet).
//!
//! See [`UnixClient`] and [`UnixConnector`] for examples.

use anyhow::{anyhow, Error};
use core::{
    pin::Pin,
    task::{Context, Poll},
};
use hex::FromHex;
use pin_project::pin_project;
use std::borrow::Cow;
use std::future::Future;
use std::path::Path;
use tokio::io::ReadBuf;

/// A type which implements `Into` for hyper's  [`hyper::Uri`] type
/// targetting unix domain sockets.
///
/// You can use this with any of
/// the HTTP factory methods on hyper's Client interface
/// and for creating requests.
///
/// ```no_run
/// extern crate hyper;
/// extern crate hyper_unix_connector;
///
/// let url: hyper::Uri = hyper_unix_connector::Uri::new(
///   "/path/to/socket", "/urlpath?key=value"
///  ).into();
///  let req = hyper::Request::get(url).body(()).unwrap();
/// ```
#[derive(Debug)]
pub struct Uri<'a> {
    /// url path including leading slash, path, and query string
    encoded: Cow<'a, str>,
}

impl<'a> Into<hyper::Uri> for Uri<'a> {
    fn into(self) -> hyper::Uri {
        self.encoded.as_ref().parse().unwrap()
    }
}

impl<'a> Uri<'a> {
    /// Productes a new `Uri` from path to domain socket and request path.
    /// request path should include a leading slash
    pub fn new<P>(socket: P, path: &'a str) -> Self
    where
        P: AsRef<Path>,
    {
        let host = hex::encode(socket.as_ref().to_string_lossy().as_bytes());
        let host_str = format!("unix://{}:0{}", host, path);
        Uri {
            encoded: Cow::Owned(host_str),
        }
    }

    // fixme: would like to just use hyper::Result and hyper::error::UriError here
    // but UriError its not exposed for external use
    fn socket_path(uri: &hyper::Uri) -> Option<String> {
        uri.host()
            .iter()
            .filter_map(|host| {
                Vec::from_hex(host)
                    .ok()
                    .map(|raw| String::from_utf8_lossy(&raw).into_owned())
            })
            .next()
    }
}

/// Wrapper around [`tokio::net::UnixListener`] that works with [`hyper`] servers.
///
/// Useful for making [`hyper`] servers listen on Unix sockets. For the client side, see
/// [`UnixClient`].
///
/// # Example
/// ```rust
/// # std::fs::remove_file("./my-unix-socket").unwrap_or_else(|_| ());
/// # let mut rt = tokio::runtime::Runtime::new().unwrap();
/// # rt.block_on(async {
/// use hyper::service::{make_service_fn, service_fn};
/// use hyper::{Body, Error, Response, Server};
/// use hyper_unix_connector::UnixConnector;
///
/// let uc: UnixConnector = tokio::net::UnixListener::bind("./my-unix-socket")
///     .unwrap()
///     .into();
/// Server::builder(uc).serve(make_service_fn(|_| {
///     async move {
///         Ok::<_, Error>(service_fn(|_| {
///             async move { Ok::<_, Error>(Response::new(Body::from("Hello, World"))) }
///         }))
///     }
/// }));
/// # });
/// # std::fs::remove_file("./my-unix-socket").unwrap_or_else(|_| ());
/// ```
#[derive(Debug)]
pub struct UnixConnector(tokio::net::UnixListener);

impl From<tokio::net::UnixListener> for UnixConnector {
    fn from(u: tokio::net::UnixListener) -> Self {
        UnixConnector(u)
    }
}

impl Into<tokio::net::UnixListener> for UnixConnector {
    fn into(self) -> tokio::net::UnixListener {
        self.0
    }
}

impl hyper::server::accept::Accept for UnixConnector {
    type Conn = tokio::net::UnixStream;
    type Error = Error;

    fn poll_accept(
        self: Pin<&mut Self>,
        cx: &mut Context,
    ) -> Poll<Option<Result<Self::Conn, Self::Error>>> {
        self.0
            .poll_accept(cx)
            .map_ok(|(stream, _addr)| stream)
            .map_err(|e| e.into())
            .map(Some)
    }
}

/// Newtype for [`tokio::net::UnixStream`] so that it can work with hyper's `Client`.
#[pin_project]
#[derive(Debug)]
pub struct UDS(#[pin] tokio::net::UnixStream);

impl From<tokio::net::UnixStream> for UDS {
    fn from(f: tokio::net::UnixStream) -> Self {
        Self(f)
    }
}

impl Into<tokio::net::UnixStream> for UDS {
    fn into(self) -> tokio::net::UnixStream {
        self.0
    }
}

macro_rules! conn_impl_fn {
    ($fn: ident |$first_var: ident: $first_typ: ty, $($var: ident: $typ: ty),*| -> $ret: ty ;;) => {
        fn $fn ($first_var: $first_typ, $( $var: $typ ),* ) -> $ret {
            let ux: Pin<&mut tokio::net::UnixStream> = $first_var.project().0;
            ux.$fn($($var),*)
        }
    };
}

impl tokio::io::AsyncRead for UDS {
    conn_impl_fn!(poll_read |self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>| -> Poll<std::io::Result<()>> ;;);
}

impl tokio::io::AsyncWrite for UDS {
    conn_impl_fn!(poll_write    |self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]| -> Poll<std::io::Result<usize>> ;;);
    conn_impl_fn!(poll_flush    |self: Pin<&mut Self>, cx: &mut Context<'_>| -> Poll<std::io::Result<()>> ;;);
    conn_impl_fn!(poll_shutdown |self: Pin<&mut Self>, cx: &mut Context<'_>| -> Poll<std::io::Result<()>> ;;);
}

/// Converts [`Uri`] to [`tokio::net::UnixStream`].
///
/// Useful for making [`hyper`] clients connect to Unix-domain addresses. For the server side, see
/// [`UnixConnector`].
///
/// # Example
/// ```rust
/// use hyper_unix_connector::{Uri, UnixClient};
/// use hyper::{Body, Client};
///
/// let client: Client<UnixClient, Body> = Client::builder().build(UnixClient);
/// let addr: hyper::Uri = Uri::new("./my_unix_socket", "/").into();
/// client.get(addr);
/// ```
#[derive(Clone, Copy, Debug)]
pub struct UnixClient;

impl hyper::service::Service<hyper::Uri> for UnixClient {
    type Response = UDS;
    type Error = Error;
    type Future =
        Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send + 'static>>;

    fn poll_ready(&mut self, _cx: &mut Context) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, dst: hyper::Uri) -> Self::Future {
        Box::pin(async move {
            match dst.scheme_str() {
                Some("unix") => (),
                _ => return Err(anyhow!("Invalid uri {:?}", dst)),
            }

            let path = match Uri::socket_path(&dst) {
                Some(path) => path,

                None => return Err(anyhow!("Invalid uri {:?}", dst)),
            };

            let st = tokio::net::UnixStream::connect(&path).await?;
            Ok(st.into())
        })
    }
}

impl hyper::client::connect::Connection for UDS {
    fn connected(&self) -> hyper::client::connect::Connected {
        hyper::client::connect::Connected::new()
    }
}

#[cfg(test)]
mod test {
    use crate::{UnixClient, UnixConnector, Uri};
    use futures_util::stream::{StreamExt, TryStreamExt};
    use hyper::service::{make_service_fn, service_fn};
    use hyper::{Body, Client, Error, Response, Server};

    #[test]
    fn ping() -> Result<(), anyhow::Error> {
        const PING_RESPONSE: &str = "Hello, World";
        const TEST_UNIX_ADDR: &str = "my-unix-socket";

        std::fs::remove_file(TEST_UNIX_ADDR).unwrap_or_else(|_| ());

        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()?;
        rt.block_on(async {
            // server
            let uc: UnixConnector = tokio::net::UnixListener::bind(TEST_UNIX_ADDR)
                .expect("bind unixlistener")
                .into();
            let srv_fut = Server::builder(uc).serve(make_service_fn(|_| async move {
                Ok::<_, Error>(service_fn(|_| async move {
                    Ok::<_, Error>(Response::new(Body::from(PING_RESPONSE)))
                }))
            }));

            // client
            let client: Client<UnixClient, Body> = Client::builder().build(UnixClient);

            tokio::spawn(async move {
                if let Err(e) = srv_fut.await {
                    panic!(e);
                }
            });

            let addr: hyper::Uri = Uri::new(TEST_UNIX_ADDR, "/").into();
            let body = client.get(addr).await.unwrap().into_body();
            let payload: Vec<u8> = body
                .map(|b| b.map(|v| v.to_vec()))
                .try_concat()
                .await
                .unwrap();
            let resp = String::from_utf8(payload).expect("body utf8");
            assert_eq!(resp, PING_RESPONSE);
        });

        std::fs::remove_file(TEST_UNIX_ADDR).unwrap_or_else(|_| ());

        Ok(())
    }
}
