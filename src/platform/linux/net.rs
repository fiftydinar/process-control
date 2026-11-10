//! The bytes each process moved over TCP.
//!
//! `/proc/<pid>/net/dev` counts an interface, not a process, and
//! `/proc/<pid>/io` counts the file system. The per-process network numbers
//! come from the sockets the process holds open: `/proc/<pid>/fd` names each
//! socket's inode, and `NETLINK_SOCK_DIAG` reports the byte counters of the
//! sockets by inode. Summing those gives what the process itself sent and
//! received, whatever it is packaged as and whichever network namespace it
//! is in.
//!
//! Only TCP is counted. A UDP socket keeps no byte counter the kernel reports,
//! so a program that streams over UDP shows nothing here rather than a lie.

use std::path::Path;

/// The byte counters one TCP socket keeps.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Socket {
    /// The socket's inode, which is how a process is matched to it.
    pub inode: u64,
    /// Bytes acknowledged from the peer.
    pub received: u64,
    /// Bytes the peer acknowledged from this side.
    pub sent: u64,
}

/// The size of one `nlmsghdr`.
const NLMSG_HEADER: usize = 16;

/// The size of one `inet_diag_req_v2`, which is all the request body is.
const REQUEST_BODY: usize = 56;

/// The size of one `inet_diag_msg`, which is the body of every answer.
const REPLY_BODY: usize = 72;

/// The offset of `idiag_inode` in `inet_diag_msg`.
const REPLY_INODE: usize = 68;

/// `SOCK_DIAG_BY_FAMILY`: the request that dumps sockets of one family.
const SOCK_DIAG_BY_FAMILY: u16 = 20;

/// `NLM_F_REQUEST | NLM_F_DUMP`: ask for the whole list in one go.
const DUMP_FLAGS: u16 = 0x0301;

/// `INET_DIAG_INFO`: include the `tcp_info` attribute.
const INET_DIAG_INFO: u8 = 1 << 1;

/// The offset of `tcpi_bytes_acked` in `tcp_info`.
const BYTES_ACKED: usize = 120;

/// The offset of `tcpi_bytes_received` in `tcp_info`.
const BYTES_RECEIVED: usize = 128;

/// How many bytes of `tcp_info` the counters at the end need.
const TCP_INFO_NEEDED: usize = BYTES_RECEIVED + 8;

/// The attribute that carries a socket's `tcp_info`.
const ATTR_TCP_INFO: u16 = 2;

/// One `nlmsghdr`.
#[repr(C)]
struct Header {
    /// Total length of the message, header included.
    len: u32,
    /// The message type.
    kind: u16,
    /// Request and dump flags.
    flags: u16,
    /// The sequence number, echoed back.
    sequence: u32,
    /// The sending process's id.
    pid: u32,
}

/// One `inet_diag_req_v2`.
#[repr(C)]
struct Request {
    /// The address family: IPv4 or IPv6.
    family: u8,
    /// The protocol, 6 for TCP.
    protocol: u8,
    /// Which attributes to include in the answer.
    extensions: u8,
    /// Padding, which the kernel wants zeroed.
    padding: u8,
    /// A bit per socket state to include; every bit asks for them all.
    states: u32,
    /// Source port, for a single-socket request.
    source_port: u16,
    /// Destination port, for a single-socket request.
    destination_port: u16,
    /// Source address, in the kernel's own byte order.
    source: [u32; 4],
    /// Destination address, in the kernel's own byte order.
    destination: [u32; 4],
    /// The interface a socket is bound to.
    interface: u32,
    /// The socket's cookie.
    cookie: [u32; 2],
}

/// Reads the TCP sockets of every namespace this process can see.
///
/// A machine with no `/proc/net` socket table, or one where the netlink
/// request is refused, gives an empty list: the column then reads zero rather
/// than the app failing.
pub fn socket_bytes() -> Option<Vec<Socket>> {
    let mut sockets = Vec::new();
    for family in [libc::AF_INET, libc::AF_INET6] {
        sockets.extend(dump(family as u8).unwrap_or_default());
    }
    Some(sockets)
}

/// Asks the kernel for the TCP sockets of `family`.
fn dump(family: u8) -> Option<Vec<Socket>> {
    // SAFETY: the socket is closed before every return.
    let descriptor =
        unsafe { libc::socket(libc::AF_NETLINK, libc::SOCK_RAW, libc::NETLINK_SOCK_DIAG) };
    if descriptor < 0 {
        return None;
    }
    let result = dump_on(descriptor, family);
    // SAFETY: `descriptor` is an open socket this process owns.
    unsafe { libc::close(descriptor) };
    Some(result)
}

/// Sends the request and reads every answer.
fn dump_on(descriptor: i32, family: u8) -> Vec<Socket> {
    // SAFETY: `sockaddr_nl` is plain data, and a zeroed one is a request to be
    // given any port.
    let mut address: libc::sockaddr_nl = unsafe { std::mem::zeroed() };
    address.nl_family = libc::AF_NETLINK as u16;
    // SAFETY: the address is a `sockaddr_nl` of the length given.
    let bound = unsafe {
        libc::bind(
            descriptor,
            (&raw const address).cast::<libc::sockaddr>(),
            std::mem::size_of::<libc::sockaddr_nl>() as libc::socklen_t,
        )
    };
    if bound < 0 {
        return Vec::new();
    }

    let mut message = vec![0u8; NLMSG_HEADER + REQUEST_BODY];
    let header = message.as_mut_ptr().cast::<Header>();
    // SAFETY: `message` is the size of both structures together, and both are
    // plain data.
    unsafe {
        (*header).len = message.len() as u32;
        (*header).kind = SOCK_DIAG_BY_FAMILY;
        (*header).flags = DUMP_FLAGS;
        (*header).sequence = 1;
        (*header).pid = 0;
        let request = message.as_mut_ptr().add(NLMSG_HEADER).cast::<Request>();
        (*request).family = family;
        (*request).protocol = 6;
        (*request).extensions = INET_DIAG_INFO;
        (*request).states = u32::MAX;
        if libc::send(descriptor, message.as_ptr().cast(), message.len(), 0) < 0 {
            return Vec::new();
        }
    }

    let mut sockets = Vec::new();
    let mut buffer = vec![0u8; 1 << 16];
    loop {
        // SAFETY: the buffer is writable and its whole length is offered.
        let read = unsafe { libc::recv(descriptor, buffer.as_mut_ptr().cast(), buffer.len(), 0) };
        if read <= 0 {
            break;
        }
        let done = parse_messages(&buffer[..read as usize], &mut sockets);
        if done {
            break;
        }
    }
    sockets
}

/// Reads every netlink message in `data`, saying whether the dump ended.
fn parse_messages(data: &[u8], sockets: &mut Vec<Socket>) -> bool {
    let mut offset = 0;
    while offset + NLMSG_HEADER <= data.len() {
        let length =
            u32::from_ne_bytes(data[offset..offset + 4].try_into().unwrap_or([0; 4])) as usize;
        let kind = u16::from_ne_bytes(data[offset + 4..offset + 6].try_into().unwrap_or([0; 2]));
        if length < NLMSG_HEADER || offset + length > data.len() {
            break;
        }
        match kind {
            // NLMSG_DONE: the dump is over.
            3 => return true,
            // NLMSG_ERROR: the kernel refused, and the rest says why, which
            // this app has nothing to do with.
            2 => return true,
            _ => {
                if let Some(socket) = parse_socket(&data[offset..offset + length]) {
                    sockets.push(socket);
                }
            }
        }
        // Messages are padded to four bytes.
        offset += (length + 3) & !3;
    }
    false
}

/// Reads one socket out of an `inet_diag_msg` and its attributes.
///
/// The body is 72 bytes: the family, the state, the timer and retransmit
/// counts, the two ports, the two addresses as sixteen bytes each, the
/// interface, the cookie, the queue lengths, the owner and the inode. The
/// byte counters follow as a `tcp_info` attribute.
fn parse_socket(message: &[u8]) -> Option<Socket> {
    let body = message.get(NLMSG_HEADER..)?;
    let inode = u32::from_ne_bytes(body.get(REPLY_INODE..REPLY_INODE + 4)?.try_into().ok()?) as u64;
    let attributes = body.get(REPLY_BODY..)?;
    let mut received = 0;
    let mut sent = 0;
    let mut offset = 0;
    while offset + 4 <= attributes.len() {
        let length = u16::from_ne_bytes(attributes[offset..offset + 2].try_into().ok()?) as usize;
        let kind = u16::from_ne_bytes(attributes[offset + 2..offset + 4].try_into().ok()?);
        if length < 4 || offset + length > attributes.len() {
            break;
        }
        let value = &attributes[offset + 4..offset + length];
        if kind == ATTR_TCP_INFO && value.len() >= TCP_INFO_NEEDED {
            sent = u64::from_ne_bytes(
                value[BYTES_ACKED..BYTES_ACKED + 8]
                    .try_into()
                    .unwrap_or([0; 8]),
            );
            received = u64::from_ne_bytes(
                value[BYTES_RECEIVED..BYTES_RECEIVED + 8]
                    .try_into()
                    .unwrap_or([0; 8]),
            );
        }
        offset += (length + 3) & !3;
    }
    Some(Socket {
        inode,
        received,
        sent,
    })
}

/// The bytes `directory`'s process moved over TCP, from the sockets it holds.
pub fn socket_bytes_of(directory: &Path, sockets: &[Socket]) -> (u64, u64) {
    let Ok(entries) = std::fs::read_dir(directory.join("fd")) else {
        return (0, 0);
    };
    let inodes: Vec<u64> = entries
        .flatten()
        .filter_map(|entry| std::fs::read_link(entry.path()).ok())
        .filter_map(|path| inode_of(&path.to_string_lossy()))
        .collect();
    if inodes.is_empty() {
        return (0, 0);
    }
    let mut received = 0;
    let mut sent = 0;
    for socket in sockets {
        if inodes.contains(&socket.inode) {
            received += socket.received;
            sent += socket.sent;
        }
    }
    (received, sent)
}

/// The inode of a `socket:[12345]` link.
fn inode_of(path: &str) -> Option<u64> {
    path.strip_prefix("socket:[")?
        .strip_suffix(']')?
        .parse()
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_socket_link_names_its_inode() {
        assert_eq!(inode_of("socket:[12345]"), Some(12345));
        assert_eq!(inode_of("/dev/null"), None);
        assert_eq!(inode_of("socket:[not a number]"), None);
    }

    #[test]
    fn the_structures_are_the_size_the_kernel_expects() {
        assert_eq!(std::mem::size_of::<Header>(), NLMSG_HEADER);
        assert_eq!(std::mem::size_of::<Request>(), REQUEST_BODY);
    }

    #[test]
    fn an_attribute_block_is_read_into_a_socket() {
        // A real `inet_diag_msg` is 72 bytes, with the inode in the last
        // four, then a `tcp_info` whose two counters sit where the kernel
        // puts them.
        let mut body = vec![0u8; REPLY_BODY];
        body[REPLY_INODE..REPLY_INODE + 4].copy_from_slice(&4242u32.to_ne_bytes());
        let mut info = vec![0u8; TCP_INFO_NEEDED];
        info[BYTES_ACKED..BYTES_ACKED + 8].copy_from_slice(&900u64.to_ne_bytes());
        info[BYTES_RECEIVED..BYTES_RECEIVED + 8].copy_from_slice(&1500u64.to_ne_bytes());
        body.extend_from_slice(&((info.len() + 4) as u16).to_ne_bytes());
        body.extend_from_slice(&ATTR_TCP_INFO.to_ne_bytes());
        body.extend_from_slice(&info);

        let mut message = vec![0u8; NLMSG_HEADER];
        message.extend_from_slice(&body);
        let socket = parse_socket(&message).expect("the message holds a socket");
        assert_eq!(
            socket,
            Socket {
                inode: 4242,
                received: 1500,
                sent: 900,
            }
        );
    }

    #[test]
    fn a_message_too_short_for_an_inode_is_left_out() {
        let message = vec![0u8; NLMSG_HEADER + REPLY_BODY - 1];
        assert!(parse_socket(&message).is_none());
    }

    #[test]
    fn a_real_tcp_socket_is_in_the_dump() {
        use std::os::fd::AsRawFd as _;

        let listener =
            std::net::TcpListener::bind(("127.0.0.1", 0)).expect("a loopback listener is made");
        let link = std::fs::read_link(format!("/proc/self/fd/{}", listener.as_raw_fd()))
            .expect("the listener has a descriptor link");
        let inode = inode_of(&link.to_string_lossy()).expect("the listener is a socket");
        let sockets = socket_bytes().expect("netlink is reachable");
        assert!(
            sockets.iter().any(|socket| socket.inode == inode),
            "the dump holds the listener's inode {inode}: {sockets:?}"
        );
    }

    #[test]
    fn a_done_message_ends_the_dump() {
        let mut data = vec![0u8; NLMSG_HEADER];
        data[0..4].copy_from_slice(&(NLMSG_HEADER as u32).to_ne_bytes());
        data[4..6].copy_from_slice(&3u16.to_ne_bytes());
        let mut sockets = Vec::new();
        assert!(parse_messages(&data, &mut sockets));
    }

    #[test]
    fn a_process_is_matched_to_its_own_sockets() {
        let directory =
            std::env::temp_dir().join(format!("process-control-sockets-{}", std::process::id()));
        let fds = directory.join("fd");
        std::fs::create_dir_all(&fds).expect("the directory is made");
        std::os::unix::fs::symlink("socket:[77]", fds.join("3")).expect("the link is made");
        std::os::unix::fs::symlink("socket:[88]", fds.join("4")).expect("the link is made");
        let sockets = [
            Socket {
                inode: 77,
                received: 10,
                sent: 20,
            },
            Socket {
                inode: 99,
                received: 1_000,
                sent: 2_000,
            },
        ];
        let (received, sent) = socket_bytes_of(&directory, &sockets);
        std::fs::remove_dir_all(&directory).expect("the temporary directory is removed");
        assert_eq!((received, sent), (10, 20));
    }
}
