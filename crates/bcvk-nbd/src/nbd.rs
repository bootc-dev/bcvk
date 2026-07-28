// NBD protocol implementation inspired by:
//   https://github.com/vi/rust-nbd (MIT/Apache-2.0)
//   https://github.com/tchajed/rust-nbd (MIT)

use std::io::{self, Read, Write};
use std::net::TcpListener;
use std::sync::Arc;
use std::thread;

use crate::regions::{self, Region};

// --- BlockDevice trait (tchajed's Blocks pattern, read-only) ---

pub trait BlockDevice: Send + Sync {
    fn read_at(&self, buf: &mut [u8], offset: u64) -> io::Result<()>;
    fn size(&self) -> u64;
}

pub struct RegionBlockDevice {
    regions: Vec<Region>,
    total_size: u64,
}

impl RegionBlockDevice {
    pub fn new(regions: Vec<Region>, total_size: u64) -> Self {
        Self {
            regions,
            total_size,
        }
    }
}

impl BlockDevice for RegionBlockDevice {
    fn read_at(&self, buf: &mut [u8], offset: u64) -> io::Result<()> {
        regions::pread(&self.regions, buf, offset)
    }

    fn size(&self) -> u64 {
        self.total_size
    }
}

// --- BigEndian I/O helpers (no byteorder dependency) ---

trait ReadBE: Read {
    fn read_u16_be(&mut self) -> io::Result<u16> {
        let mut buf = [0u8; 2];
        self.read_exact(&mut buf)?;
        Ok(u16::from_be_bytes(buf))
    }
    fn read_u32_be(&mut self) -> io::Result<u32> {
        let mut buf = [0u8; 4];
        self.read_exact(&mut buf)?;
        Ok(u32::from_be_bytes(buf))
    }
    fn read_u64_be(&mut self) -> io::Result<u64> {
        let mut buf = [0u8; 8];
        self.read_exact(&mut buf)?;
        Ok(u64::from_be_bytes(buf))
    }
}
impl<R: Read> ReadBE for R {}

trait WriteBE: Write {
    fn write_u16_be(&mut self, v: u16) -> io::Result<()> {
        self.write_all(&v.to_be_bytes())
    }
    fn write_u32_be(&mut self, v: u32) -> io::Result<()> {
        self.write_all(&v.to_be_bytes())
    }
    fn write_u64_be(&mut self, v: u64) -> io::Result<()> {
        self.write_all(&v.to_be_bytes())
    }
}
impl<W: Write> WriteBE for W {}

// --- NBD protocol constants ---

const MAGIC: &[u8; 8] = b"NBDMAGIC";
const IHAVEOPT: u64 = 0x49484156454F5054;
const REPLY_MAGIC: u64 = 0x3e889045565a9;
const REQUEST_MAGIC: u32 = 0x25609513;
const SIMPLE_REPLY_MAGIC: u32 = 0x67446698;

const NBD_FLAG_FIXED_NEWSTYLE: u16 = 1 << 0;
const NBD_FLAG_NO_ZEROES: u16 = 1 << 1;

const NBD_FLAG_HAS_FLAGS: u16 = 1 << 0;
const NBD_FLAG_READ_ONLY: u16 = 1 << 1;
const NBD_FLAG_CAN_MULTI_CONN: u16 = 1 << 8;

const NBD_FLAG_C_FIXED_NEWSTYLE: u32 = 1;
const NBD_FLAG_C_NO_ZEROES: u32 = 2;

const NBD_OPT_EXPORT_NAME: u32 = 1;
const NBD_OPT_ABORT: u32 = 2;
const NBD_OPT_LIST: u32 = 3;
const NBD_OPT_INFO: u32 = 6;
const NBD_OPT_GO: u32 = 7;

const NBD_REP_ACK: u32 = 1;
const NBD_REP_SERVER: u32 = 2;
const NBD_REP_INFO: u32 = 3;
const NBD_REP_ERR_UNSUP: u32 = (1 << 31) | 1;

const NBD_INFO_EXPORT: u16 = 0;
const NBD_INFO_BLOCK_SIZE: u16 = 3;

const NBD_CMD_READ: u16 = 0;
const NBD_CMD_DISC: u16 = 2;

const TRANSMIT_FLAGS: u16 = NBD_FLAG_HAS_FLAGS | NBD_FLAG_READ_ONLY | NBD_FLAG_CAN_MULTI_CONN;

// --- Server ---

pub fn serve_tcp<B: BlockDevice + 'static>(listener: TcpListener, device: B) {
    let device = Arc::new(device);
    for stream in listener.incoming() {
        let stream = match stream {
            Ok(s) => s,
            Err(e) => {
                eprintln!("nbd: accept error: {e}");
                continue;
            }
        };
        let _ = stream.set_nodelay(true);
        let dev = Arc::clone(&device);
        thread::spawn(move || {
            if let Err(e) = handle_connection(stream, &*dev) {
                if e.kind() != io::ErrorKind::UnexpectedEof {
                    eprintln!("nbd: connection error: {e}");
                }
            }
        });
    }
}

pub fn serve_vsock<B: BlockDevice + 'static>(port: u32, device: B) {
    let fd = unsafe { libc::socket(libc::AF_VSOCK, libc::SOCK_STREAM, 0) };
    if fd < 0 {
        eprintln!("nbd: failed to create vsock socket");
        std::process::exit(1);
    }

    let mut addr: libc::sockaddr_vm = unsafe { std::mem::zeroed() };
    addr.svm_family = libc::AF_VSOCK as _;
    addr.svm_port = port;
    addr.svm_cid = libc::VMADDR_CID_ANY;
    let ret = unsafe {
        libc::bind(
            fd,
            &addr as *const libc::sockaddr_vm as *const libc::sockaddr,
            std::mem::size_of::<libc::sockaddr_vm>() as libc::socklen_t,
        )
    };
    if ret < 0 {
        eprintln!("nbd: vsock bind failed on port {port}");
        std::process::exit(1);
    }

    let ret = unsafe { libc::listen(fd, 4) };
    if ret < 0 {
        eprintln!("nbd: vsock listen failed");
        std::process::exit(1);
    }

    eprintln!("nbd: listening on vsock port {port}");

    let device = Arc::new(device);
    loop {
        let client_fd = unsafe { libc::accept(fd, std::ptr::null_mut(), std::ptr::null_mut()) };
        if client_fd < 0 {
            eprintln!("nbd: vsock accept error");
            continue;
        }
        let dev = Arc::clone(&device);
        thread::spawn(move || {
            use std::os::unix::io::FromRawFd;
            let stream = unsafe { std::net::TcpStream::from_raw_fd(client_fd) };
            if let Err(e) = handle_connection(stream, &*dev) {
                if e.kind() != io::ErrorKind::UnexpectedEof {
                    eprintln!("nbd: vsock connection error: {e}");
                }
            }
        });
    }
}

fn handle_connection<S: Read + Write, B: BlockDevice>(mut stream: S, device: &B) -> io::Result<()> {
    handshake(&mut stream, device)?;
    transmission(&mut stream, device)
}

// --- Handshake (vi/rust-nbd base + tchajed's OPT_GO) ---

fn handshake<S: Read + Write, B: BlockDevice>(s: &mut S, device: &B) -> io::Result<()> {
    s.write_all(MAGIC)?;
    s.write_u64_be(IHAVEOPT)?;
    s.write_u16_be(NBD_FLAG_FIXED_NEWSTYLE | NBD_FLAG_NO_ZEROES)?;
    s.flush()?;

    let client_flags = s.read_u32_be()?;
    if client_flags & NBD_FLAG_C_FIXED_NEWSTYLE == 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "client must support FIXED_NEWSTYLE",
        ));
    }
    let no_zeroes = client_flags & NBD_FLAG_C_NO_ZEROES != 0;

    loop {
        let opt_magic = s.read_u64_be()?;
        if opt_magic != IHAVEOPT {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid option magic",
            ));
        }
        let opt = s.read_u32_be()?;
        let opt_len = s.read_u32_be()?;
        if opt_len > 100_000 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "option data too large",
            ));
        }
        let mut opt_data = vec![0u8; opt_len as usize];
        s.read_exact(&mut opt_data)?;

        match opt {
            NBD_OPT_EXPORT_NAME => {
                send_export_reply(s, device, no_zeroes)?;
                return Ok(());
            }
            NBD_OPT_GO => {
                send_info_export(s, opt, device)?;
                send_info_block_size(s, opt)?;
                opt_reply(s, opt, NBD_REP_ACK, &[])?;
                return Ok(());
            }
            NBD_OPT_INFO => {
                send_info_export(s, opt, device)?;
                send_info_block_size(s, opt)?;
                opt_reply(s, opt, NBD_REP_ACK, &[])?;
            }
            NBD_OPT_LIST => {
                let name = b"bcvk";
                let mut data = (name.len() as u32).to_be_bytes().to_vec();
                data.extend_from_slice(name);
                opt_reply(s, opt, NBD_REP_SERVER, &data)?;
                opt_reply(s, opt, NBD_REP_ACK, &[])?;
            }
            NBD_OPT_ABORT => {
                opt_reply(s, opt, NBD_REP_ACK, &[])?;
                return Err(io::Error::new(
                    io::ErrorKind::ConnectionAborted,
                    "client abort",
                ));
            }
            _ => {
                opt_reply(s, opt, NBD_REP_ERR_UNSUP, &[])?;
            }
        }
    }
}

fn send_export_reply<S: Write, B: BlockDevice>(
    s: &mut S,
    device: &B,
    no_zeroes: bool,
) -> io::Result<()> {
    s.write_u64_be(device.size())?;
    s.write_u16_be(TRANSMIT_FLAGS)?;
    if !no_zeroes {
        s.write_all(&[0u8; 124])?;
    }
    s.flush()
}

fn send_info_export<S: Write, B: BlockDevice>(s: &mut S, opt: u32, device: &B) -> io::Result<()> {
    let mut data = Vec::with_capacity(12);
    data.extend_from_slice(&NBD_INFO_EXPORT.to_be_bytes());
    data.extend_from_slice(&device.size().to_be_bytes());
    data.extend_from_slice(&TRANSMIT_FLAGS.to_be_bytes());
    opt_reply(s, opt, NBD_REP_INFO, &data)
}

fn send_info_block_size<S: Write>(s: &mut S, opt: u32) -> io::Result<()> {
    let mut data = Vec::with_capacity(14);
    data.extend_from_slice(&NBD_INFO_BLOCK_SIZE.to_be_bytes());
    data.extend_from_slice(&1u32.to_be_bytes()); // minimum
    data.extend_from_slice(&4096u32.to_be_bytes()); // preferred
    data.extend_from_slice(&(4096u32 * 32).to_be_bytes()); // maximum
    opt_reply(s, opt, NBD_REP_INFO, &data)
}

fn opt_reply<S: Write>(s: &mut S, opt: u32, reply_type: u32, data: &[u8]) -> io::Result<()> {
    s.write_u64_be(REPLY_MAGIC)?;
    s.write_u32_be(opt)?;
    s.write_u32_be(reply_type)?;
    s.write_u32_be(data.len() as u32)?;
    s.write_all(data)?;
    s.flush()
}

// --- Transmission (vi/rust-nbd dispatch, pread-native) ---

fn transmission<S: Read + Write, B: BlockDevice>(s: &mut S, device: &B) -> io::Result<()> {
    let mut buf = vec![0u8; 256 * 1024];
    loop {
        let magic = s.read_u32_be()?;
        if magic != REQUEST_MAGIC {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid request magic",
            ));
        }
        let _flags = s.read_u16_be()?;
        let cmd = s.read_u16_be()?;
        let handle = s.read_u64_be()?;
        let offset = s.read_u64_be()?;
        let length = s.read_u32_be()?;

        match cmd {
            NBD_CMD_READ => {
                let len = length as usize;
                if buf.len() < len {
                    buf.resize(len, 0);
                }
                match device.read_at(&mut buf[..len], offset) {
                    Ok(()) => {
                        simple_reply(s, 0, handle)?;
                        s.write_all(&buf[..len])?;
                        s.flush()?;
                    }
                    Err(e) => {
                        let errno = e.raw_os_error().unwrap_or(5) as u32;
                        simple_reply(s, errno, handle)?;
                        s.flush()?;
                    }
                }
            }
            NBD_CMD_DISC => {
                return Ok(());
            }
            _ => {
                // Read-only: reject WRITE, FLUSH, TRIM, etc.
                simple_reply(s, 95, handle)?; // ENOTSUP
                s.flush()?;
            }
        }
    }
}

fn simple_reply<S: Write>(s: &mut S, error: u32, handle: u64) -> io::Result<()> {
    s.write_u32_be(SIMPLE_REPLY_MAGIC)?;
    s.write_u32_be(error)?;
    s.write_u64_be(handle)
}
