//! Connections between launchers. Each side proves it is signed in to the same account,
//! then the client asks for manifests and files. A server only ever hands out files of
//! instances with sync turned on; it never accepts files, so a peer can't write here.

use std::fs::File;
use std::io::{BufReader, BufWriter, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use super::identity::{SyncIdentity, random_id};
use super::manifest::{self, Manifest};
use super::protocol::{self, Message};
use super::rules;
use super::Device;
use crate::instance::Instance;
use crate::{Error, Result};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(4);
const IO_TIMEOUT: Duration = Duration::from_secs(60);

/// What the server needs from the launcher.
pub trait Host: Send + Sync + 'static {
    /// The account this launcher syncs for; `None` while signed out.
    fn identity(&self) -> Option<SyncIdentity>;
    fn device(&self) -> Device;
    /// The instance with sync id `sync_id`, unless it is missing or in use right now.
    fn shared_instance(&self, sync_id: &str) -> Result<Instance>;
    /// The peer `from` has newer files of `sync_id`.
    fn changed(&self, from: &Device, sync_id: &str);
}

/// Accepts connections on `listener` until the program exits.
pub fn serve(listener: TcpListener, host: Arc<dyn Host>) {
    thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let host = host.clone();
            thread::spawn(move || {
                if let Err(e) = handle(stream, &*host) {
                    eprintln!("lan sync: {e}");
                }
            });
        }
    });
}

fn handle(stream: TcpStream, host: &dyn Host) -> Result<()> {
    stream.set_read_timeout(Some(IO_TIMEOUT))?;
    stream.set_write_timeout(Some(IO_TIMEOUT))?;
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut writer = BufWriter::new(stream);
    let Some(identity) = host.identity() else {
        return protocol::send(&mut writer, &Message::Error { message: "not signed in".into() });
    };
    let me = host.device();
    let nonce = random_id();
    protocol::send(
        &mut writer,
        &Message::Hello { version: protocol::VERSION, device_id: me.id.clone(), device_name: me.name, nonce: nonce.clone() },
    )?;
    let Message::Auth { device_id, device_name, nonce: theirs, proof } = protocol::receive(&mut reader)? else {
        return Err(Error::msg("expected auth"));
    };
    if !same(&proof, &identity.proof("client", &nonce, &theirs)) || device_id == me.id {
        return protocol::send(&mut writer, &Message::Error { message: "different account".into() });
    }
    protocol::send(&mut writer, &Message::Welcome { proof: identity.proof("server", &theirs, &nonce) })?;
    let peer = Device { id: device_id, name: device_name };

    loop {
        let request = match protocol::receive(&mut reader) {
            Ok(request) => request,
            // The client hung up.
            Err(Error::Io(_)) => return Ok(()),
            Err(e) => return Err(e),
        };
        let reply = match request {
            Message::GetManifest { sync_id } => {
                host.shared_instance(&sync_id).and_then(|i| manifest::scan(&i)).map(|manifest| Message::Manifest { manifest })
            }
            Message::GetFile { sync_id, path } => match open(host, &sync_id, &path) {
                Ok((mut file, size)) => {
                    protocol::send_file(&mut writer, &mut (&mut file).take(size), size)?;
                    continue;
                }
                Err(e) => Err(e),
            },
            Message::Changed { sync_id } => {
                host.changed(&peer, &sync_id);
                Ok(Message::Ok)
            }
            _ => Err(Error::msg("unexpected request")),
        };
        let reply = reply.unwrap_or_else(|e| Message::Error { message: e.to_string() });
        protocol::send(&mut writer, &reply)?;
    }
}

/// Opens a synced file of a shared instance, refusing anything outside it.
fn open(host: &dyn Host, sync_id: &str, path: &str) -> Result<(File, u64)> {
    let instance = host.shared_instance(sync_id)?;
    rules::check(path)?;
    let full = rules::on_disk(&instance.dir, path);
    if rules::crosses_link(&instance.dir, &full) {
        return Err(Error::msg(format!("refusing to send '{path}'")));
    }
    let file = File::open(&full)?;
    let size = file.metadata()?.len();
    Ok((file, size))
}

/// Compares proofs without stopping at the first difference.
fn same(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// A connection to another launcher of the same account.
pub struct Connection {
    reader: BufReader<TcpStream>,
    writer: BufWriter<TcpStream>,
    pub peer: Device,
}

impl Connection {
    pub fn open(addr: SocketAddr, identity: &SyncIdentity, me: &Device) -> Result<Self> {
        let stream = TcpStream::connect_timeout(&addr, CONNECT_TIMEOUT)?;
        stream.set_read_timeout(Some(IO_TIMEOUT))?;
        stream.set_write_timeout(Some(IO_TIMEOUT))?;
        stream.set_nodelay(true)?;
        let mut reader = BufReader::new(stream.try_clone()?);
        let mut writer = BufWriter::new(stream);
        let Message::Hello { version, device_id, device_name, nonce } = protocol::expect(&mut reader)? else {
            return Err(Error::msg("unexpected greeting"));
        };
        if version != protocol::VERSION {
            return Err(Error::msg(format!("{device_name} runs a different version of Axi Launcher")));
        }
        let mine = random_id();
        protocol::send(
            &mut writer,
            &Message::Auth {
                device_id: me.id.clone(),
                device_name: me.name.clone(),
                nonce: mine.clone(),
                proof: identity.proof("client", &nonce, &mine),
            },
        )?;
        let Message::Welcome { proof } = protocol::expect(&mut reader)? else {
            return Err(Error::msg("unexpected reply"));
        };
        if !same(&proof, &identity.proof("server", &mine, &nonce)) {
            return Err(Error::msg(format!("{device_name} is signed in to a different account")));
        }
        Ok(Self { reader, writer, peer: Device { id: device_id, name: device_name } })
    }

    pub fn manifest(&mut self, sync_id: &str) -> Result<Manifest> {
        protocol::send(&mut self.writer, &Message::GetManifest { sync_id: sync_id.into() })?;
        match protocol::expect(&mut self.reader)? {
            Message::Manifest { manifest } => Ok(manifest),
            _ => Err(Error::msg("unexpected reply")),
        }
    }

    /// Writes the peer's copy of `path` into `out`.
    pub fn fetch(&mut self, sync_id: &str, path: &str, out: &mut impl Write) -> Result<()> {
        protocol::send(&mut self.writer, &Message::GetFile { sync_id: sync_id.into(), path: path.into() })?;
        match protocol::expect(&mut self.reader)? {
            Message::File { size } => protocol::receive_file(&mut self.reader, out, size),
            _ => Err(Error::msg("unexpected reply")),
        }
    }

    /// Tells the peer this computer has newer files of `sync_id`.
    pub fn changed(&mut self, sync_id: &str) -> Result<()> {
        protocol::send(&mut self.writer, &Message::Changed { sync_id: sync_id.into() })?;
        protocol::expect(&mut self.reader).map(|_| ())
    }
}
