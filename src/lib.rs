//! Connect hyper servers and clients to Unix-domain sockets.
//!
//! Most of this crate's functionality is borrowed from [hyperlocal](https://github.com/softprops/hyperlocal).
//! This crate supports async/await, while hyperlocal does not (yet).
//!
//! See [`UnixClient`] and [`UnixConnector`] for examples.

use core::{
    pin::Pin,
    task::{Context, Poll},
};
use failure::Error;
use futures_util::future::FutureExt;
use futures_util::try_future::TryFutureExt;
use hex::FromHex;
use std::borrow::Cow;
use std::future::Future;
use std::path::Path;

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

    fn socket_path_dest(dest: &hyper::client::connect::Destination) -> Option<String> {
        format!("unix://{}", dest.host())
            .parse()
            .ok()
            .and_then(|uri| Self::socket_path(&uri))
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
/// # std::fs::remove_file("./my-unix-socket").unwrap_or_else(|_| ());
/// ```
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
        mut self: Pin<&mut Self>,
        cx: &mut Context,
    ) -> Poll<Option<Result<Self::Conn, Self::Error>>> {
        let fut = self
            .0
            .accept()
            .map_ok(|(stream, _addr)| stream)
            .map_err(|e| e.into())
            .map(|f| Some(f));
        Future::poll(Box::pin(fut).as_mut(), cx)
    }
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
pub struct UnixClient;

impl hyper::client::connect::Connect for UnixClient {
    type Transport = tokio::net::UnixStream;
    type Error = Error;
    type Future = Pin<
        Box<
            dyn Future<Output = Result<(Self::Transport, hyper::client::connect::Connected), Error>>
                + Send,
        >,
    >;

    fn connect(&self, dst: hyper::client::connect::Destination) -> Self::Future {
        Box::pin(async move {
            if dst.scheme() != "unix" {
                return Err(failure::format_err!("Invalid uri {:?}", dst));
            }

            let path = match Uri::socket_path_dest(&dst) {
                Some(path) => path,

                None => return Err(failure::format_err!("Invalid uri {:?}", dst)),
            };

            let st = tokio::net::UnixStream::connect(&path).await?;
            Ok((st, hyper::client::connect::Connected::new()))
        })
    }
}

impl hyper::client::service::Service<hyper::Uri> for UnixClient {
    type Response = tokio::net::UnixStream;
    type Error = Error;
    type Future =
        Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send + 'static>>;

    fn poll_ready(&mut self, _cx: &mut Context) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, uri: hyper::Uri) -> Self::Future {
        Box::pin(async move {
            let dest = hyper::client::connect::Destination::try_from_uri(uri)?;
            use hyper::client::connect::Connect;
            let u = UnixClient;
            let (uc, _) = u.connect(dest).await?;
            Ok(uc)
        })
    }
}

#[cfg(test)]
mod test {
    use crate::{UnixClient, UnixConnector, Uri};
    use futures_util::try_stream::TryStreamExt;
    use hyper::service::{make_service_fn, service_fn};
    use hyper::{Body, Client, Error, Response, Server};

    #[test]
    fn ping() -> Result<(), failure::Error> {
        const PING_RESPONSE: &str = "Hello, World";
        const TEST_UNIX_ADDR: &str = "my-unix-socket";

        std::fs::remove_file(TEST_UNIX_ADDR).unwrap_or_else(|_| ());

        // server
        let uc: UnixConnector = tokio::net::UnixListener::bind(TEST_UNIX_ADDR)
            .unwrap()
            .into();
        let srv_fut = Server::builder(uc).serve(make_service_fn(|_| {
            async move {
                Ok::<_, Error>(service_fn(|_| {
                    async move { Ok::<_, Error>(Response::new(Body::from(PING_RESPONSE))) }
                }))
            }
        }));

        // client
        let client: Client<UnixClient, Body> = Client::builder().build(UnixClient);

        let mut rt = tokio::runtime::current_thread::Runtime::new()?;
        let resp = rt.block_on(async move {
            tokio::spawn(async move {
                if let Err(e) = srv_fut.await {
                    panic!(e);
                }
            });

            let addr: hyper::Uri = Uri::new(TEST_UNIX_ADDR, "/").into();
            let body = client.get(addr).await.unwrap().into_body();
            let payload: hyper::Chunk = body.try_concat().await.unwrap();
            String::from_utf8(payload.to_vec()).expect("body utf8")
        });

        assert_eq!(resp, PING_RESPONSE);
        std::fs::remove_file(TEST_UNIX_ADDR).unwrap_or_else(|_| ());

        Ok(())
    }
}
