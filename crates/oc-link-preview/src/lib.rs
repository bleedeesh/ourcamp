//! What a link leads to, for a card in a chat (ADR-0077). The sender's app asks, so a page
//! learns no more than that someone opened a link from this address, and a camp asks nothing.
//!
//! Everything here treats the internet as hostile: only public addresses (checked after name
//! resolution and on every redirect, then connected to directly), limits on size and time.

mod address;
mod html;

use std::io;
use std::net::SocketAddr;
use std::time::Duration;

use bytes::Bytes;
use http_body_util::{BodyExt, Empty};
use hyper::body::Incoming;
use hyper::header::{
    ACCEPT, ACCEPT_ENCODING, ACCEPT_LANGUAGE, CONTENT_TYPE, HOST, LOCATION, USER_AGENT,
};
use hyper::{Request, Response};
use hyper_util::rt::TokioIo;
use oc_tls::Trust;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;
use url::Url;

pub use address::is_public;
pub use html::{charset_of, read_page, PageInfo};

/// The whole of reading a page, redirects included.
const PAGE_TIMEOUT: Duration = Duration::from_secs(8);
/// The whole of fetching its picture: they are heavier, and a card can go without.
const PICTURE_TIMEOUT: Duration = Duration::from_secs(12);
/// Each attempt to connect, of a name's several addresses.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(4);
const MAX_REDIRECTS: usize = 5;
/// Of a page: heads come first, and the rest of it is not needed.
const MAX_PAGE_BYTES: usize = 1024 * 1024;
/// Of a picture: what a camp keeps at most, a GIF.
const MAX_PICTURE_BYTES: usize = oc_proto::MAX_GIF_BYTES;
/// Sites answer differently to what looks like nothing; this says what it is.
const AGENT: &str = "Mozilla/5.0 (compatible; OurCamp; link preview)";

/// A card's content, before its picture goes through the window (ADR-0077).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Preview {
    /// Up to [`oc_proto::MAX_LINK_TITLE_CHARS`]; may be empty with a picture.
    pub title: String,
    /// Up to [`oc_proto::MAX_LINK_DESCRIPTION_CHARS`].
    pub description: String,
    /// The picture as it came: the window decodes it.
    pub picture: Option<Vec<u8>>,
}

/// Why there is no preview. Nobody is told: a link simply goes without a card.
#[derive(Debug)]
pub enum PreviewError {
    /// Not an address of the web, or one with a login in it.
    BadUrl,
    /// It leads to a computer that is not on the open internet.
    NotPublic,
    /// Too many redirects, or one that makes no sense.
    Redirects,
    /// Neither a page nor a picture, or one saying nothing.
    NothingToShow,
    /// The site cannot be reached or answered with an error.
    Unreachable(io::Error),
}

impl From<io::Error> for PreviewError {
    fn from(e: io::Error) -> Self {
        Self::Unreachable(e)
    }
}

/// Fetches a preview of `link`.
///
/// # Errors
/// See [`PreviewError`].
pub async fn preview(link: &str) -> Result<Preview, PreviewError> {
    Fetcher {
        allowed: is_public,
        trust: Trust::of(None),
    }
    .preview(link)
    .await
}

/// A fetch's rules. Tests bring their own `allowed`: they have only loopback to talk to.
struct Fetcher {
    allowed: fn(std::net::IpAddr) -> bool,
    trust: Trust,
}

/// A successful answer: the address it finally came from, its `Content-Type`, its body.
struct Fetched {
    url: Url,
    content_type: String,
    body: Vec<u8>,
}

/// A link is fetchable only if it is `http(s)` and carries no login: a link like
/// `https://bank@evil.example` is made to mislead, and a password must not be sent anywhere.
fn parse(link: &str) -> Result<Url, PreviewError> {
    let url = Url::parse(link).map_err(|_| PreviewError::BadUrl)?;
    let web = matches!(url.scheme(), "http" | "https");
    if !web || !url.username().is_empty() || url.password().is_some() || url.host().is_none() {
        return Err(PreviewError::BadUrl);
    }
    Ok(url)
}

/// The address to hand to a browser, in its plain form: `None` for anything but the web and
/// for addresses with a login in them.
#[must_use]
pub fn web_address(link: &str) -> Option<String> {
    parse(link).ok().map(String::from)
}

fn is_picture(content_type: &str) -> bool {
    media_type(content_type).starts_with("image/")
}

fn is_page(content_type: &str) -> bool {
    matches!(
        media_type(content_type).as_str(),
        "text/html" | "application/xhtml+xml"
    )
}

fn media_type(content_type: &str) -> String {
    content_type
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase()
}

/// At most `max` characters, ending in "…" when something was cut.
fn cut(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_owned();
    }
    let kept: String = text.chars().take(max.saturating_sub(1)).collect();
    format!("{}…", kept.trim_end())
}

impl Fetcher {
    async fn preview(&self, link: &str) -> Result<Preview, PreviewError> {
        let timeout = |e: tokio::time::error::Elapsed| {
            PreviewError::Unreachable(io::Error::new(io::ErrorKind::TimedOut, e))
        };
        // The picture may be asked for straight away: then it is the whole of it.
        let page = tokio::time::timeout(
            PAGE_TIMEOUT,
            self.fetch(
                parse(link)?,
                "text/html,application/xhtml+xml,image/gif,image/*;q=0.8",
                MAX_PAGE_BYTES,
            ),
        )
        .await
        .map_err(timeout)??;

        if is_picture(&page.content_type) {
            // Cut at the page's limit, it is fetched again in full.
            let picture = if page.body.len() < MAX_PAGE_BYTES {
                page.body
            } else {
                self.picture(page.url.as_str()).await?
            };
            return Ok(Preview {
                picture: Some(picture),
                ..Preview::default()
            });
        }
        if !is_page(&page.content_type) {
            return Err(PreviewError::NothingToShow);
        }
        let info = read_page(&page.body, charset_of(&page.content_type));
        // Relative to where the page finally was. A picture that fails costs only itself.
        let picture = match page.url.join(&info.image) {
            Ok(image) if !info.image.is_empty() => self.picture(image.as_str()).await.ok(),
            _ => None,
        };
        if info.title.is_empty() && picture.is_none() {
            return Err(PreviewError::NothingToShow);
        }
        Ok(Preview {
            title: cut(&info.title, oc_proto::MAX_LINK_TITLE_CHARS),
            description: cut(&info.description, oc_proto::MAX_LINK_DESCRIPTION_CHARS),
            picture,
        })
    }

    async fn picture(&self, link: &str) -> Result<Vec<u8>, PreviewError> {
        let fetched = tokio::time::timeout(
            PICTURE_TIMEOUT,
            // A GIF first: sites offering a choice would rather send what does not move.
            self.fetch(
                parse(link)?,
                "image/gif,image/*;q=0.8",
                MAX_PICTURE_BYTES + 1,
            ),
        )
        .await
        .map_err(|e| PreviewError::Unreachable(io::Error::new(io::ErrorKind::TimedOut, e)))??;
        if !is_picture(&fetched.content_type) || fetched.body.len() > MAX_PICTURE_BYTES {
            return Err(PreviewError::NothingToShow);
        }
        Ok(fetched.body)
    }

    /// `GET`, following redirects, each checked like the first.
    async fn fetch(
        &self,
        mut url: Url,
        accept: &str,
        limit: usize,
    ) -> Result<Fetched, PreviewError> {
        for _ in 0..=MAX_REDIRECTS {
            let response = self.get(&url, accept).await?;
            let status = response.status();
            if status.is_redirection() {
                let location = response
                    .headers()
                    .get(LOCATION)
                    .and_then(|value| value.to_str().ok())
                    .ok_or(PreviewError::Redirects)?;
                let next = url.join(location).map_err(|_| PreviewError::Redirects)?;
                url = parse(next.as_str())?;
                continue;
            }
            if !status.is_success() {
                return Err(io::Error::other(format!("HTTP {status}")).into());
            }
            // Asked for none; a server that compresses anyway cannot be read.
            if response
                .headers()
                .get(hyper::header::CONTENT_ENCODING)
                .is_some_and(|e| e != "identity")
            {
                return Err(PreviewError::NothingToShow);
            }
            let content_type = response
                .headers()
                .get(CONTENT_TYPE)
                .and_then(|value| value.to_str().ok())
                .unwrap_or("")
                .to_owned();
            let body = read_up_to(response.into_body(), limit).await?;
            return Ok(Fetched {
                url,
                content_type,
                body,
            });
        }
        Err(PreviewError::Redirects)
    }

    /// One request over a fresh connection to a public address of `url`'s host.
    async fn get(&self, url: &Url, accept: &str) -> Result<Response<Incoming>, PreviewError> {
        let host = url.host_str().ok_or(PreviewError::BadUrl)?;
        let port = url.port_or_known_default().ok_or(PreviewError::BadUrl)?;
        let tcp = self.connect(host, port).await?;
        let authority = url
            .port()
            .map_or_else(|| host.to_owned(), |port| format!("{host}:{port}"));
        let path = &url[url::Position::BeforePath..url::Position::AfterQuery];
        let request = Request::builder()
            .uri(path)
            .header(HOST, authority)
            .header(USER_AGENT, AGENT)
            .header(ACCEPT, accept)
            .header(ACCEPT_LANGUAGE, "ru,en;q=0.8")
            .header(ACCEPT_ENCODING, "identity")
            .body(Empty::<Bytes>::new())
            .map_err(|e| io::Error::other(e.to_string()))?;
        if url.scheme() == "https" {
            let name = self
                .trust
                .server_name(host)
                .map_err(|_| PreviewError::BadUrl)?;
            let tls = TlsConnector::from(self.trust.client_config())
                .connect(name, tcp)
                .await?;
            exchange(tls, request).await
        } else {
            exchange(tcp, request).await
        }
    }

    /// Resolves first and connects to the very addresses checked, so a name cannot change its
    /// answer in between. One private address among public ones refuses the name.
    async fn connect(&self, host: &str, port: u16) -> Result<TcpStream, PreviewError> {
        let host = host.trim_start_matches('[').trim_end_matches(']');
        let addresses: Vec<SocketAddr> = tokio::net::lookup_host((host, port)).await?.collect();
        if addresses.is_empty() || !addresses.iter().all(|a| (self.allowed)(a.ip())) {
            return Err(PreviewError::NotPublic);
        }
        let mut last = io::Error::from(io::ErrorKind::NotConnected);
        for address in addresses {
            match tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect(address)).await {
                Ok(Ok(tcp)) => return Ok(tcp),
                Ok(Err(e)) => last = e,
                Err(e) => last = io::Error::new(io::ErrorKind::TimedOut, e),
            }
        }
        Err(last.into())
    }
}

async fn exchange<S>(
    stream: S,
    request: Request<Empty<Bytes>>,
) -> Result<Response<Incoming>, PreviewError>
where
    S: AsyncRead + AsyncWrite + Send + Unpin + 'static,
{
    let (mut sender, connection) = hyper::client::conn::http1::handshake(TokioIo::new(stream))
        .await
        .map_err(io::Error::other)?;
    // Drives the connection; ends when the response is read and the request dropped.
    tokio::spawn(connection);
    Ok(sender
        .send_request(request)
        .await
        .map_err(io::Error::other)?)
}

/// The body up to `limit` bytes; the rest is not read.
async fn read_up_to(mut body: Incoming, limit: usize) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    while let Some(frame) = body.frame().await {
        if let Ok(data) = frame.map_err(io::Error::other)?.into_data() {
            bytes.extend_from_slice(&data);
            if bytes.len() >= limit {
                bytes.truncate(limit);
                break;
            }
        }
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use std::net::IpAddr;

    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    use super::*;

    /// Serves one answer per request, chosen by the path, and says what it was asked.
    async fn site(answer: fn(&str, u16) -> Vec<u8>) -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            loop {
                let (mut tcp, _) = listener.accept().await.unwrap();
                tokio::spawn(async move {
                    let mut request = vec![0; 4096];
                    let n = tcp.read(&mut request).await.unwrap_or(0);
                    let request = String::from_utf8_lossy(&request[..n]).into_owned();
                    let path = request.split_whitespace().nth(1).unwrap_or("/").to_owned();
                    let _ = tcp.write_all(&answer(&path, port)).await;
                    let _ = tcp.shutdown().await;
                });
            }
        });
        port
    }

    fn http(status: &str, headers: &str, body: &[u8]) -> Vec<u8> {
        let head = format!(
            "HTTP/1.1 {status}\r\n{headers}Content-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        [head.as_bytes(), body].concat()
    }

    const GIF: &[u8] = b"GIF89a-a-picture";

    fn answers(path: &str, port: u16) -> Vec<u8> {
        let html = "Content-Type: text/html; charset=utf-8\r\n";
        match path {
            "/page" => http(
                "200 OK",
                html,
                "<title>Заголовок</title><meta name=description content=Описание><meta property=og:image content=/a.gif>".as_bytes(),
            ),
            "/a.gif" => http("200 OK", "Content-Type: image/gif\r\n", GIF),
            "/no-picture" => http(
                "200 OK",
                html,
                "<title>Заголовок</title><meta property=og:image content=/missing.png>".as_bytes(),
            ),
            "/empty" => http("200 OK", html, b"<p>nothing</p>"),
            "/zip" => http("200 OK", "Content-Type: application/zip\r\n", b"PK"),
            "/gzip" => http("200 OK", "Content-Type: text/html\r\nContent-Encoding: gzip\r\n", b"x"),
            "/moved" => http("302 Found", "Location: /page\r\n", b""),
            "/moved-full" => http(
                "301 Moved",
                &format!("Location: http://127.0.0.1:{port}/page\r\n"),
                b"",
            ),
            "/to-private" => http(
                "302 Found",
                &format!("Location: http://127.0.0.2:{port}/page\r\n"),
                b"",
            ),
            "/to-file" => http("302 Found", "Location: file:///C:/secret\r\n", b""),
            "/loop" => http("302 Found", "Location: /loop\r\n", b""),
            "/huge.gif" => http(
                "200 OK",
                "Content-Type: image/gif\r\n",
                &vec![b'g'; MAX_PICTURE_BYTES + 1],
            ),
            "/huge-picture" => http(
                "200 OK",
                html,
                b"<title>T</title><meta property=og:image content=/huge.gif>",
            ),
            "/long" => http(
                "200 OK",
                html,
                format!(
                    "<title>{}</title><meta name=description content='{}'>",
                    "щ".repeat(500),
                    "ж".repeat(500)
                )
                .as_bytes(),
            ),
            _ => http("404 Not Found", html, b"gone"),
        }
    }

    /// As in production, but 127.0.0.1 is "the internet"; 127.0.0.2 stays private.
    fn fetcher() -> Fetcher {
        Fetcher {
            allowed: |ip| ip == IpAddr::from([127, 0, 0, 1]),
            trust: Trust::of(None),
        }
    }

    async fn at(port: u16, path: &str) -> Result<Preview, PreviewError> {
        fetcher()
            .preview(&format!("http://127.0.0.1:{port}{path}"))
            .await
    }

    #[tokio::test]
    async fn a_page_gives_its_title_description_and_picture() {
        let port = site(answers).await;
        let expected = Preview {
            title: "Заголовок".into(),
            description: "Описание".into(),
            picture: Some(GIF.to_vec()),
        };
        assert_eq!(at(port, "/page").await.unwrap(), expected);
        assert_eq!(at(port, "/moved").await.unwrap(), expected, "relative");
        assert_eq!(at(port, "/moved-full").await.unwrap(), expected, "absolute");
    }

    #[tokio::test]
    async fn a_picture_is_its_own_preview() {
        let port = site(answers).await;
        assert_eq!(
            at(port, "/a.gif").await.unwrap(),
            Preview {
                picture: Some(GIF.to_vec()),
                ..Preview::default()
            }
        );
    }

    #[tokio::test]
    async fn a_card_goes_on_without_its_picture() {
        let port = site(answers).await;
        for path in ["/no-picture", "/huge-picture"] {
            let card = at(port, path).await.unwrap();
            assert!(!card.title.is_empty(), "{path}");
            assert_eq!(card.picture, None, "{path}");
        }
    }

    #[tokio::test]
    async fn long_texts_are_cut_to_what_a_camp_takes() {
        let port = site(answers).await;
        let card = at(port, "/long").await.unwrap();
        assert_eq!(card.title.chars().count(), oc_proto::MAX_LINK_TITLE_CHARS);
        assert!(card.title.ends_with('…'));
        assert_eq!(
            card.description.chars().count(),
            oc_proto::MAX_LINK_DESCRIPTION_CHARS
        );
    }

    #[tokio::test]
    async fn nothing_to_show_is_no_preview() {
        let port = site(answers).await;
        for path in ["/empty", "/zip", "/gzip"] {
            assert!(
                matches!(at(port, path).await, Err(PreviewError::NothingToShow)),
                "{path}"
            );
        }
        assert!(matches!(
            at(port, "/nowhere").await,
            Err(PreviewError::Unreachable(_))
        ));
    }

    #[tokio::test]
    async fn redirects_are_checked_and_counted() {
        let port = site(answers).await;
        assert!(matches!(
            at(port, "/to-private").await,
            Err(PreviewError::NotPublic)
        ));
        assert!(matches!(
            at(port, "/to-file").await,
            Err(PreviewError::BadUrl)
        ));
        assert!(matches!(
            at(port, "/loop").await,
            Err(PreviewError::Redirects)
        ));
    }

    #[tokio::test]
    async fn only_public_addresses_of_the_web_are_fetched() {
        for link in [
            "http://127.0.0.1/",
            "http://localhost/",
            "http://[::1]/",
            "http://192.168.1.1/",
            "http://169.254.169.254/latest/meta-data/",
        ] {
            assert!(
                matches!(preview(link).await, Err(PreviewError::NotPublic)),
                "{link}"
            );
        }
        for link in [
            "ftp://example.com/",
            "file:///C:/Windows/win.ini",
            "javascript:alert(1)",
            "https://bank@evil.example/",
            "https://user:password@example.com/",
            "not a link",
        ] {
            assert!(
                matches!(preview(link).await, Err(PreviewError::BadUrl)),
                "{link}"
            );
        }
    }
}
