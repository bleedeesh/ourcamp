//! UPnP Internet Gateway Device, the little of it a camp needs (ADR-0039): find the home
//! router, ask its public address, open and close the camp's ports on it.
//!
//! ```text
//! SSDP M-SEARCH (UDP multicast) ─► LOCATION: http://router/desc.xml
//!   GET desc.xml ─► WANIPConnection (or WANPPPConnection) controlURL
//!   SOAP POST controlURL: GetExternalIPAddress · AddPortMapping · DeletePortMapping
//! ```
//!
//! Written here rather than taken from a crate (ADR-0007): plain HTTP/1.1 to the LAN, and
//! a few XML elements read by name — no general XML parser is needed for them.

use std::fmt::{self, Write as _};
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::time::Duration;

use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::header::{CONTENT_TYPE, HOST};
use hyper::{Method, Request};
use hyper_util::rt::TokioIo;
use tokio::net::{TcpStream, UdpSocket};

/// Where routers listen for SSDP searches.
const SSDP: SocketAddrV4 = SocketAddrV4::new(Ipv4Addr::new(239, 255, 255, 250), 1900);
/// What is searched for: the gateway device, both versions.
const SEARCH_TARGETS: [&str; 2] = [
    "urn:schemas-upnp-org:device:InternetGatewayDevice:2",
    "urn:schemas-upnp-org:device:InternetGatewayDevice:1",
];
/// The services that map ports, best first.
const SERVICES: [&str; 3] = [
    "urn:schemas-upnp-org:service:WANIPConnection:2",
    "urn:schemas-upnp-org:service:WANIPConnection:1",
    "urn:schemas-upnp-org:service:WANPPPConnection:1",
];
/// One HTTP exchange with the router must answer within this.
const CALL_TIMEOUT: Duration = Duration::from_secs(5);
/// The router's description is small; anything bigger is no router of ours.
const MAX_BODY: usize = 256 * 1024;

/// Why UPnP did not work.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpnpError {
    /// No router answered the search: UPnP is off on it, or it has none.
    NoGateway,
    /// The router answered, but not as a gateway that maps ports.
    NotAGateway(String),
    /// No answer, or not HTTP.
    Unreachable(String),
    /// The router refused: its UPnP error code and words.
    Refused {
        /// UPnP error code (718: the port is taken by another device).
        code: u16,
        /// The router's own words.
        description: String,
    },
}

impl fmt::Display for UpnpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoGateway => write!(f, "no UPnP router answered"),
            Self::NotAGateway(why) => write!(f, "not a UPnP gateway: {why}"),
            Self::Unreachable(why) => write!(f, "router unreachable: {why}"),
            Self::Refused { code, description } => {
                write!(f, "router refused ({code}): {description}")
            }
        }
    }
}

impl std::error::Error for UpnpError {}

/// A port's protocol.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Protocol {
    /// TCP: the camp's API and voice over TCP.
    Tcp,
    /// UDP: voice.
    Udp,
}

impl Protocol {
    fn as_str(self) -> &'static str {
        match self {
            Self::Tcp => "TCP",
            Self::Udp => "UDP",
        }
    }
}

// ---- reading the router's XML ----

/// The text of the first element named `name` (namespace prefix ignored), unescaped.
fn element<'a>(xml: &'a str, name: &str) -> Option<&'a str> {
    let mut rest = xml;
    loop {
        let open = rest.find('<')?;
        rest = &rest[open + 1..];
        let end = rest.find('>')?;
        let tag = &rest[..end];
        let local = tag.split_whitespace().next()?;
        let local = local.rsplit(':').next()?;
        if local == name && !tag.starts_with('/') && !tag.ends_with('/') {
            let body = &rest[end + 1..];
            let close = body.find("</")?;
            return Some(body[..close].trim());
        }
        rest = &rest[end + 1..];
    }
}

/// Every block between `<name>` and `</name>` (to look inside each `<service>`).
fn blocks<'a>(xml: &'a str, name: &str) -> Vec<&'a str> {
    let (open, close) = (format!("<{name}>"), format!("</{name}>"));
    let mut out = Vec::new();
    let mut rest = xml;
    while let Some(start) = rest.find(&open) {
        let inner = &rest[start + open.len()..];
        let Some(end) = inner.find(&close) else { break };
        out.push(&inner[..end]);
        rest = &inner[end + close.len()..];
    }
    out
}

fn unescape(text: &str) -> String {
    text.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

/// `LOCATION:` of an SSDP answer.
fn location(answer: &str) -> Option<&str> {
    answer.lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        name.trim()
            .eq_ignore_ascii_case("location")
            .then(|| value.trim())
    })
}

/// An `http://host:port/path` URL, the only kind a router hands out.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Url {
    authority: SocketAddr,
    path: String,
}

impl Url {
    fn parse(text: &str) -> Option<Self> {
        let rest = text.strip_prefix("http://")?;
        let (authority, path) = rest.split_once('/').map_or((rest, ""), |(a, p)| (a, p));
        let authority: SocketAddr = if authority.contains(':') {
            authority.parse().ok()?
        } else {
            SocketAddr::from((authority.parse::<Ipv4Addr>().ok()?, 80))
        };
        Some(Self {
            authority,
            path: format!("/{path}"),
        })
    }

    /// A URL from the description, which may be relative to the description's own.
    fn join(&self, reference: &str) -> Option<Self> {
        if reference.starts_with("http://") {
            return Self::parse(reference);
        }
        let path = if reference.starts_with('/') {
            reference.to_owned()
        } else {
            let base = self.path.rsplit_once('/').map_or("", |(dir, _)| dir);
            format!("{base}/{reference}")
        };
        Some(Self {
            authority: self.authority,
            path,
        })
    }
}

/// The port-mapping service in a device description: its type and control URL.
fn find_service(description: &str, base: &Url) -> Option<(&'static str, Url)> {
    let base = element(description, "URLBase")
        .and_then(Url::parse)
        .unwrap_or_else(|| base.clone());
    let services = blocks(description, "service");
    SERVICES.iter().find_map(|wanted| {
        services.iter().find_map(|service| {
            (element(service, "serviceType") == Some(*wanted))
                .then(|| element(service, "controlURL"))
                .flatten()
                .and_then(|control| base.join(&unescape(control)))
                .map(|url| (*wanted, url))
        })
    })
}

/// The error code and words of a SOAP fault.
fn fault(body: &str) -> Option<(u16, String)> {
    let code = element(body, "errorCode")?.parse().ok()?;
    let description = element(body, "errorDescription")
        .map(unescape)
        .unwrap_or_default();
    Some((code, description))
}

// ---- talking to the router ----

/// One HTTP/1.1 request; the status, the body, and our address on that connection.
async fn http(
    url: &Url,
    method: Method,
    headers: &[(&str, String)],
    body: String,
) -> Result<(u16, String, Ipv4Addr), UpnpError> {
    let unreachable = |e: &dyn fmt::Display| UpnpError::Unreachable(e.to_string());
    let exchange = async {
        let tcp = TcpStream::connect(url.authority)
            .await
            .map_err(|e| unreachable(&e))?;
        let local = match tcp.local_addr().map_err(|e| unreachable(&e))?.ip() {
            std::net::IpAddr::V4(ip) => ip,
            std::net::IpAddr::V6(_) => return Err(unreachable(&"IPv6 router")),
        };
        let (mut sender, connection) = hyper::client::conn::http1::handshake(TokioIo::new(tcp))
            .await
            .map_err(|e| unreachable(&e))?;
        tokio::spawn(connection);
        let mut request = Request::builder()
            .method(method)
            .uri(&url.path)
            .header(HOST, url.authority.to_string());
        for (name, value) in headers {
            request = request.header(*name, value);
        }
        let request = request
            .body(Full::new(Bytes::from(body)))
            .map_err(|e| unreachable(&e))?;
        let response = sender
            .send_request(request)
            .await
            .map_err(|e| unreachable(&e))?;
        let status = response.status().as_u16();
        let bytes = http_body_util::Limited::new(response.into_body(), MAX_BODY)
            .collect()
            .await
            .map_err(|e| unreachable(&e))?
            .to_bytes();
        Ok((status, String::from_utf8_lossy(&bytes).into_owned(), local))
    };
    tokio::time::timeout(CALL_TIMEOUT, exchange)
        .await
        .map_err(|_| unreachable(&"timed out"))?
}

/// The home router, found and ready to map ports.
#[derive(Debug, Clone)]
pub struct Gateway {
    control: Url,
    service: &'static str,
    /// This PC's address towards the router: where mapped ports lead.
    pub local_ip: Ipv4Addr,
}

impl Gateway {
    /// Looks for the router on the local network for up to `wait`.
    ///
    /// # Errors
    /// [`UpnpError::NoGateway`] when none answers (UPnP off), others when one answers
    /// badly.
    pub async fn discover(wait: Duration) -> Result<Self, UpnpError> {
        // Bound to the main network's address: with VPN and virtual adapters (Radmin,
        // Docker, WSL) the OS may send multicast out of any of them otherwise.
        let local = crate::network::default_address().unwrap_or(Ipv4Addr::UNSPECIFIED);
        let socket = UdpSocket::bind((local, 0))
            .await
            .map_err(|e| UpnpError::Unreachable(e.to_string()))?;
        for target in SEARCH_TARGETS {
            let search = format!(
                "M-SEARCH * HTTP/1.1\r\nHOST: {SSDP}\r\nMAN: \"ssdp:discover\"\r\nMX: 2\r\nST: {target}\r\n\r\n"
            );
            socket
                .send_to(search.as_bytes(), SSDP)
                .await
                .map_err(|e| UpnpError::Unreachable(e.to_string()))?;
        }
        let deadline = tokio::time::Instant::now() + wait;
        let mut last = UpnpError::NoGateway;
        let mut buf = [0u8; 2048];
        // Several devices may answer (a TV, a printer): the first that is a gateway wins.
        while let Ok(Ok((n, _))) =
            tokio::time::timeout_at(deadline, socket.recv_from(&mut buf)).await
        {
            let answer = String::from_utf8_lossy(&buf[..n]);
            let Some(url) = location(&answer).and_then(Url::parse) else {
                continue;
            };
            match Self::describe(&url).await {
                Ok(gateway) => return Ok(gateway),
                Err(e) => last = e,
            }
        }
        Err(last)
    }

    async fn describe(url: &Url) -> Result<Self, UpnpError> {
        let (status, body, local_ip) = http(url, Method::GET, &[], String::new()).await?;
        if status != 200 {
            return Err(UpnpError::NotAGateway(format!("description HTTP {status}")));
        }
        let (service, control) = find_service(&body, url)
            .ok_or_else(|| UpnpError::NotAGateway("no WANIPConnection service".into()))?;
        Ok(Self {
            control,
            service,
            local_ip,
        })
    }

    async fn call(&self, action: &str, arguments: &[(&str, String)]) -> Result<String, UpnpError> {
        let args = arguments
            .iter()
            .fold(String::new(), |mut out, (name, value)| {
                let _ = write!(out, "<{name}>{value}</{name}>");
                out
            });
        let body = format!(
            "<?xml version=\"1.0\"?>\
             <s:Envelope xmlns:s=\"http://schemas.xmlsoap.org/soap/envelope/\" \
             s:encodingStyle=\"http://schemas.xmlsoap.org/soap/encoding/\">\
             <s:Body><u:{action} xmlns:u=\"{service}\">{args}</u:{action}></s:Body></s:Envelope>",
            service = self.service
        );
        let headers = [
            (
                CONTENT_TYPE.as_str(),
                "text/xml; charset=\"utf-8\"".to_owned(),
            ),
            ("SOAPAction", format!("\"{}#{action}\"", self.service)),
        ];
        let (status, body, _) = http(&self.control, Method::POST, &headers, body).await?;
        if status == 200 {
            return Ok(body);
        }
        let (code, description) = fault(&body).unwrap_or((status, format!("HTTP {status}")));
        Err(UpnpError::Refused { code, description })
    }

    /// The router's public address.
    ///
    /// # Errors
    /// The router cannot be reached, refuses, or answers without an address.
    pub async fn external_ip(&self) -> Result<Ipv4Addr, UpnpError> {
        let body = self.call("GetExternalIPAddress", &[]).await?;
        element(&body, "NewExternalIPAddress")
            .and_then(|ip| ip.parse().ok())
            .ok_or_else(|| UpnpError::NotAGateway("no external address in the answer".into()))
    }

    /// Opens `port` on the router to this PC for `lease` (0: until removed). Routers
    /// that only keep permanent mappings (error 725) get one.
    ///
    /// # Errors
    /// The router refuses (718: the port is taken by another device) or is unreachable.
    pub async fn add_port(
        &self,
        protocol: Protocol,
        port: u16,
        lease: Duration,
    ) -> Result<(), UpnpError> {
        let request = |lease_secs: u64| {
            [
                ("NewRemoteHost", String::new()),
                ("NewExternalPort", port.to_string()),
                ("NewProtocol", protocol.as_str().to_owned()),
                ("NewInternalPort", port.to_string()),
                ("NewInternalClient", self.local_ip.to_string()),
                ("NewEnabled", "1".to_owned()),
                ("NewPortMappingDescription", "OurCamp".to_owned()),
                ("NewLeaseDuration", lease_secs.to_string()),
            ]
        };
        match self.call("AddPortMapping", &request(lease.as_secs())).await {
            Err(UpnpError::Refused { code: 725, .. }) => {
                self.call("AddPortMapping", &request(0)).await.map(drop)
            }
            other => other.map(drop),
        }
    }

    /// Closes `port`. One that is not open is no error.
    ///
    /// # Errors
    /// The router refuses or is unreachable.
    pub async fn remove_port(&self, protocol: Protocol, port: u16) -> Result<(), UpnpError> {
        let args = [
            ("NewRemoteHost", String::new()),
            ("NewExternalPort", port.to_string()),
            ("NewProtocol", protocol.as_str().to_owned()),
        ];
        match self.call("DeletePortMapping", &args).await {
            // 714: NoSuchEntryInArray.
            Ok(_) | Err(UpnpError::Refused { code: 714, .. }) => Ok(()),
            Err(e) => Err(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A FRITZ!Box-like description: nested devices, a relative control URL, no `URLBase`.
    const DESCRIPTION: &str = r#"<?xml version="1.0"?>
<root xmlns="urn:schemas-upnp-org:device-1-0">
 <device><deviceType>urn:schemas-upnp-org:device:InternetGatewayDevice:1</deviceType>
  <serviceList><service>
    <serviceType>urn:schemas-upnp-org:service:Layer3Forwarding:1</serviceType>
    <controlURL>/upnp/control/layer3forwarding</controlURL></service></serviceList>
  <deviceList><device><deviceType>urn:schemas-upnp-org:device:WANDevice:1</deviceType>
   <deviceList><device><deviceType>urn:schemas-upnp-org:device:WANConnectionDevice:1</deviceType>
    <serviceList><service>
      <serviceType>urn:schemas-upnp-org:service:WANIPConnection:1</serviceType>
      <serviceId>urn:upnp-org:serviceId:WANIPConn1</serviceId>
      <controlURL>ctl/IPConn?x=1&amp;y=2</controlURL>
    </service></serviceList>
   </device></deviceList>
  </device></deviceList>
 </device>
</root>"#;

    fn url(text: &str) -> Url {
        Url::parse(text).unwrap()
    }

    #[test]
    fn the_port_mapping_service_is_found_in_the_description() {
        let base = url("http://192.168.1.1:49000/igd/desc.xml");
        let (service, control) = find_service(DESCRIPTION, &base).unwrap();
        assert_eq!(service, "urn:schemas-upnp-org:service:WANIPConnection:1");
        assert_eq!(
            control,
            url("http://192.168.1.1:49000/igd/ctl/IPConn?x=1&y=2"),
            "relative, unescaped"
        );
        let ppp = DESCRIPTION.replace("WANIPConnection:1", "WANPPPConnection:1");
        assert_eq!(
            find_service(&ppp, &base).unwrap().0,
            "urn:schemas-upnp-org:service:WANPPPConnection:1",
            "DSL routers"
        );
        assert!(
            find_service("<root><device/></root>", &base).is_none(),
            "a printer"
        );
    }

    #[test]
    fn a_url_base_or_an_absolute_control_url_wins() {
        let base = url("http://192.168.1.1:5000/rootDesc.xml");
        let with_base = DESCRIPTION.replace(
            "<device>",
            "<URLBase>http://192.168.1.1:2869/</URLBase><device>",
        );
        assert_eq!(
            find_service(&with_base, &base).unwrap().1,
            url("http://192.168.1.1:2869/ctl/IPConn?x=1&y=2")
        );
        assert_eq!(base.join("/ctl"), Some(url("http://192.168.1.1:5000/ctl")));
        assert_eq!(
            base.join("http://10.0.0.1:80/c"),
            Some(url("http://10.0.0.1/c"))
        );
        assert_eq!(
            Url::parse("http://192.168.0.1/"),
            Some(url("http://192.168.0.1:80/"))
        );
        assert_eq!(Url::parse("https://router/"), None);
    }

    #[test]
    fn ssdp_answers_and_soap_replies_are_read() {
        let answer = "HTTP/1.1 200 OK\r\nCACHE-CONTROL: max-age=120\r\nLocation: http://192.168.1.1:5000/rootDesc.xml\r\nST: urn:schemas-upnp-org:device:InternetGatewayDevice:1\r\n\r\n";
        assert_eq!(
            location(answer),
            Some("http://192.168.1.1:5000/rootDesc.xml")
        );
        assert_eq!(location("HTTP/1.1 200 OK\r\n\r\n"), None);

        let reply = r#"<s:Envelope><s:Body><u:GetExternalIPAddressResponse xmlns:u="x">
            <NewExternalIPAddress>85.143.20.11</NewExternalIPAddress></u:GetExternalIPAddressResponse></s:Body></s:Envelope>"#;
        assert_eq!(element(reply, "NewExternalIPAddress"), Some("85.143.20.11"));

        let refusal = r#"<s:Envelope><s:Body><s:Fault><faultcode>s:Client</faultcode><detail>
            <UPnPError xmlns="urn:schemas-upnp-org:control-1-0"><errorCode>718</errorCode>
            <errorDescription>ConflictInMappingEntry</errorDescription></UPnPError></detail></s:Fault></s:Body></s:Envelope>"#;
        assert_eq!(fault(refusal), Some((718, "ConflictInMappingEntry".into())));
        assert_eq!(
            element("<a><b/><b>x</b></a>", "b"),
            Some("x"),
            "empty elements are skipped"
        );
    }

    /// A router on loopback that keeps only permanent mappings, like many cheap ones.
    async fn fake_router() -> (String, std::sync::Arc<std::sync::Mutex<Vec<String>>>) {
        use axum::extract::State;
        use axum::http::{HeaderMap, StatusCode};
        use axum::routing::{get, post};

        type Mapped = std::sync::Arc<std::sync::Mutex<Vec<String>>>;
        let mapped: Mapped = std::sync::Arc::default();
        let description = DESCRIPTION.replace("ctl/IPConn?x=1&amp;y=2", "/ctl");
        let refuse = |code: u16, words: &str| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!(
                    "<s:Envelope><s:Body><s:Fault><detail><UPnPError><errorCode>{code}</errorCode>\
                     <errorDescription>{words}</errorDescription></UPnPError></detail></s:Fault></s:Body></s:Envelope>"
                ),
            )
        };
        let control = move |State(mapped): State<Mapped>, headers: HeaderMap, body: String| async move {
            let action = headers["soapaction"].to_str().unwrap().to_owned();
            let port = element(&body, "NewExternalPort")
                .unwrap_or_default()
                .to_owned();
            let entry = format!(
                "{}/{port}",
                element(&body, "NewProtocol").unwrap_or_default()
            );
            let mut mapped = mapped.lock().unwrap();
            if action.ends_with("#GetExternalIPAddress\"") {
                (
                    StatusCode::OK,
                    "<r><NewExternalIPAddress>85.143.20.11</NewExternalIPAddress></r>".into(),
                )
            } else if action.ends_with("#AddPortMapping\"") {
                if element(&body, "NewLeaseDuration") != Some("0") {
                    return refuse(725, "OnlyPermanentLeasesSupported");
                }
                mapped.push(entry);
                (StatusCode::OK, String::new())
            } else if mapped.contains(&entry) {
                mapped.retain(|m| *m != entry);
                (StatusCode::OK, String::new())
            } else {
                refuse(714, "NoSuchEntryInArray")
            }
        };
        let app = axum::Router::new()
            .route("/desc.xml", get(move || async move { description }))
            .route("/ctl", post(control))
            .with_state(std::sync::Arc::clone(&mapped));
        let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await });
        (format!("http://{addr}/desc.xml"), mapped)
    }

    #[tokio::test]
    async fn the_router_tells_its_address_and_maps_ports() {
        let (location, mapped) = fake_router().await;
        let gateway = Gateway::describe(&url(&location)).await.unwrap();
        assert_eq!(
            gateway.local_ip,
            Ipv4Addr::LOCALHOST,
            "where the router reaches us"
        );
        assert_eq!(
            gateway.external_ip().await.unwrap(),
            Ipv4Addr::new(85, 143, 20, 11)
        );

        gateway
            .add_port(Protocol::Tcp, 7780, Duration::from_secs(3600))
            .await
            .unwrap();
        gateway
            .add_port(Protocol::Udp, 7882, Duration::from_secs(3600))
            .await
            .unwrap();
        assert_eq!(
            *mapped.lock().unwrap(),
            ["TCP/7780", "UDP/7882"],
            "permanent after a 725"
        );

        gateway.remove_port(Protocol::Tcp, 7780).await.unwrap();
        gateway.remove_port(Protocol::Tcp, 7780).await.unwrap();
        assert_eq!(
            *mapped.lock().unwrap(),
            ["UDP/7882"],
            "a second close is no error"
        );
    }

    /// This network's router, read only: found, and its public address. Maps nothing.
    #[tokio::test]
    #[ignore = "talks to this network's router (read-only)"]
    async fn this_networks_router() {
        match Gateway::discover(Duration::from_secs(3)).await {
            Ok(gateway) => {
                eprintln!(
                    "router: {} ({}), this PC {}",
                    gateway.control.authority, gateway.service, gateway.local_ip
                );
                eprintln!("external address: {:?}", gateway.external_ip().await);
            }
            Err(e) => eprintln!("no router: {e}"),
        }
    }
}
