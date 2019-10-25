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

/// A type which implements `Into` for hyper's  `hyper::Uri` type
/// targetting unix domain sockets.
///
/// You can use this with any of
/// the HTTP factory methods on hyper's Client interface
/// and for creating requests.
///
/// Borrowed from [hyperlocal](https://github.com/softprops/hyperlocal).
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

pub struct UnixConnector(tokio::net::UnixListener);

impl From<tokio::net::UnixListener> for UnixConnector {
    fn from(u: tokio::net::UnixListener) -> Self {
        UnixConnector(u)
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
