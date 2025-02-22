//! eBPF program types.
//!
//! eBPF programs are loaded inside the kernel and attached to one or more hook
//! points. Whenever the hook points are reached, the programs are executed.
//!
//! # Loading and attaching programs
//!
//! When you call [`Bpf::load_file`] or [`Bpf::load`], all the programs included
//! in the object code are parsed and relocated. Programs are not loaded
//! automatically though, since often you will need to do some application
//! specific setup before you can actually load them.
//!
//! In order to load and attach a program, you need to retrieve it using [`Bpf::program_mut`],
//! then call the `load()` and `attach()` methods, for example:
//!
//! ```no_run
//! use aya::{Bpf, programs::KProbe};
//!
//! let mut bpf = Bpf::load_file("ebpf_programs.o")?;
//! // intercept_wakeups is the name of the program we want to load
//! let program: &mut KProbe = bpf.program_mut("intercept_wakeups").unwrap().try_into()?;
//! program.load()?;
//! // intercept_wakeups will be called every time try_to_wake_up() is called
//! // inside the kernel
//! program.attach("try_to_wake_up", 0)?;
//! # Ok::<(), aya::BpfError>(())
//! ```
//!
//! The signature of the `attach()` method varies depending on what kind of
//! program you're trying to attach.
//!
//! [`Bpf::load_file`]: crate::Bpf::load_file
//! [`Bpf::load`]: crate::Bpf::load
//! [`Bpf::programs`]: crate::Bpf::programs
//! [`Bpf::program`]: crate::Bpf::program
//! [`Bpf::program_mut`]: crate::Bpf::program_mut
//! [`maps`]: crate::maps
pub mod cgroup_device;
pub mod cgroup_skb;
pub mod cgroup_sock;
pub mod cgroup_sock_addr;
pub mod cgroup_sockopt;
pub mod cgroup_sysctl;
pub mod extension;
pub mod fentry;
pub mod fexit;
pub mod kprobe;
pub mod links;
pub mod lirc_mode2;
pub mod lsm;
pub mod perf_attach;
pub mod perf_event;
mod probe;
mod raw_trace_point;
mod sk_lookup;
mod sk_msg;
mod sk_skb;
mod sock_ops;
mod socket_filter;
pub mod tc;
pub mod tp_btf;
pub mod trace_point;
pub mod uprobe;
mod utils;
pub mod xdp;

use std::{
    ffi::CString,
    io,
    num::NonZeroU32,
    os::fd::{AsFd, AsRawFd, BorrowedFd, OwnedFd},
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, SystemTime},
};

pub use cgroup_device::CgroupDevice;
pub use cgroup_skb::{CgroupSkb, CgroupSkbAttachType};
pub use cgroup_sock::{CgroupSock, CgroupSockAttachType};
pub use cgroup_sock_addr::{CgroupSockAddr, CgroupSockAddrAttachType};
pub use cgroup_sockopt::{CgroupSockopt, CgroupSockoptAttachType};
pub use cgroup_sysctl::CgroupSysctl;
pub use extension::{Extension, ExtensionError};
pub use fentry::FEntry;
pub use fexit::FExit;
pub use kprobe::{KProbe, KProbeError};
use libc::ENOSPC;
pub use links::Link;
use links::*;
pub use lirc_mode2::LircMode2;
pub use lsm::Lsm;
use perf_attach::*;
pub use perf_event::{PerfEvent, PerfEventScope, PerfTypeId, SamplePolicy};
pub use probe::ProbeKind;
pub use raw_trace_point::RawTracePoint;
pub use sk_lookup::SkLookup;
pub use sk_msg::SkMsg;
pub use sk_skb::{SkSkb, SkSkbKind};
pub use sock_ops::SockOps;
pub use socket_filter::{SocketFilter, SocketFilterError};
pub use tc::{SchedClassifier, TcAttachType, TcError};
use thiserror::Error;
pub use tp_btf::BtfTracePoint;
pub use trace_point::{TracePoint, TracePointError};
pub use uprobe::{UProbe, UProbeError};
pub use xdp::{Xdp, XdpError, XdpFlags};

use crate::{
    generated::{bpf_attach_type, bpf_link_info, bpf_prog_info, bpf_prog_type},
    maps::MapError,
    obj::{self, btf::BtfError, VerifierLog},
    pin::PinError,
    programs::utils::{boot_time, get_fdinfo},
    sys::{
        bpf_btf_get_fd_by_id, bpf_get_object, bpf_link_get_fd_by_id, bpf_link_get_info_by_fd,
        bpf_load_program, bpf_pin_object, bpf_prog_get_fd_by_id, bpf_prog_get_info_by_fd,
        bpf_prog_query, iter_link_ids, iter_prog_ids, retry_with_verifier_logs,
        BpfLoadProgramAttrs, SyscallError,
    },
    util::KernelVersion,
    VerifierLogLevel,
};

/// Error type returned when working with programs.
#[derive(Debug, Error)]
pub enum ProgramError {
    /// The program is already loaded.
    #[error("the program is already loaded")]
    AlreadyLoaded,

    /// The program is not loaded.
    #[error("the program is not loaded")]
    NotLoaded,

    /// The program is already attached.
    #[error("the program was already attached")]
    AlreadyAttached,

    /// The program is not attached.
    #[error("the program is not attached")]
    NotAttached,

    /// Loading the program failed.
    #[error("the BPF_PROG_LOAD syscall failed. Verifier output: {verifier_log}")]
    LoadError {
        /// The [`io::Error`] returned by the `BPF_PROG_LOAD` syscall.
        #[source]
        io_error: io::Error,
        /// The error log produced by the kernel verifier.
        verifier_log: VerifierLog,
    },

    /// A syscall failed.
    #[error(transparent)]
    SyscallError(#[from] SyscallError),

    /// The network interface does not exist.
    #[error("unknown network interface {name}")]
    UnknownInterface {
        /// interface name
        name: String,
    },

    /// The program is not of the expected type.
    #[error("unexpected program type")]
    UnexpectedProgramType,

    /// A map error occurred while loading or attaching a program.
    #[error(transparent)]
    MapError(#[from] MapError),

    /// An error occurred while working with a [`KProbe`].
    #[error(transparent)]
    KProbeError(#[from] KProbeError),

    /// An error occurred while working with an [`UProbe`].
    #[error(transparent)]
    UProbeError(#[from] UProbeError),

    /// An error occurred while working with a [`TracePoint`].
    #[error(transparent)]
    TracePointError(#[from] TracePointError),

    /// An error occurred while working with a [`SocketFilter`].
    #[error(transparent)]
    SocketFilterError(#[from] SocketFilterError),

    /// An error occurred while working with an [`Xdp`] program.
    #[error(transparent)]
    XdpError(#[from] XdpError),

    /// An error occurred while working with a TC program.
    #[error(transparent)]
    TcError(#[from] TcError),

    /// An error occurred while working with an [`Extension`] program.
    #[error(transparent)]
    ExtensionError(#[from] ExtensionError),

    /// An error occurred while working with BTF.
    #[error(transparent)]
    Btf(#[from] BtfError),

    /// The program is not attached.
    #[error("the program name `{name}` is invalid")]
    InvalidName {
        /// program name
        name: String,
    },

    /// An error occurred while working with IO.
    #[error(transparent)]
    IOError(#[from] io::Error),
}

/// A [`Program`] file descriptor.
#[derive(Debug)]
pub struct ProgramFd(OwnedFd);

impl ProgramFd {
    /// Creates a new instance that shares the same underlying file description as [`self`].
    pub fn try_clone(&self) -> io::Result<Self> {
        let Self(inner) = self;
        let inner = inner.try_clone()?;
        Ok(Self(inner))
    }
}

impl AsFd for ProgramFd {
    fn as_fd(&self) -> BorrowedFd<'_> {
        let Self(fd) = self;
        fd.as_fd()
    }
}

/// eBPF program type.
#[derive(Debug)]
pub enum Program {
    /// A [`KProbe`] program
    KProbe(KProbe),
    /// A [`UProbe`] program
    UProbe(UProbe),
    /// A [`TracePoint`] program
    TracePoint(TracePoint),
    /// A [`SocketFilter`] program
    SocketFilter(SocketFilter),
    /// A [`Xdp`] program
    Xdp(Xdp),
    /// A [`SkMsg`] program
    SkMsg(SkMsg),
    /// A [`SkSkb`] program
    SkSkb(SkSkb),
    /// A [`CgroupSockAddr`] program
    CgroupSockAddr(CgroupSockAddr),
    /// A [`SockOps`] program
    SockOps(SockOps),
    /// A [`SchedClassifier`] program
    SchedClassifier(SchedClassifier),
    /// A [`CgroupSkb`] program
    CgroupSkb(CgroupSkb),
    /// A [`CgroupSysctl`] program
    CgroupSysctl(CgroupSysctl),
    /// A [`CgroupSockopt`] program
    CgroupSockopt(CgroupSockopt),
    /// A [`LircMode2`] program
    LircMode2(LircMode2),
    /// A [`PerfEvent`] program
    PerfEvent(PerfEvent),
    /// A [`RawTracePoint`] program
    RawTracePoint(RawTracePoint),
    /// A [`Lsm`] program
    Lsm(Lsm),
    /// A [`BtfTracePoint`] program
    BtfTracePoint(BtfTracePoint),
    /// A [`FEntry`] program
    FEntry(FEntry),
    /// A [`FExit`] program
    FExit(FExit),
    /// A [`Extension`] program
    Extension(Extension),
    /// A [`SkLookup`] program
    SkLookup(SkLookup),
    /// A [`CgroupSock`] program
    CgroupSock(CgroupSock),
    /// A [`CgroupDevice`] program
    CgroupDevice(CgroupDevice),
}

impl Program {
    /// Returns the low level program type.
    pub fn prog_type(&self) -> bpf_prog_type {
        use crate::generated::bpf_prog_type::*;
        match self {
            Self::KProbe(_) => BPF_PROG_TYPE_KPROBE,
            Self::UProbe(_) => BPF_PROG_TYPE_KPROBE,
            Self::TracePoint(_) => BPF_PROG_TYPE_TRACEPOINT,
            Self::SocketFilter(_) => BPF_PROG_TYPE_SOCKET_FILTER,
            Self::Xdp(_) => BPF_PROG_TYPE_XDP,
            Self::SkMsg(_) => BPF_PROG_TYPE_SK_MSG,
            Self::SkSkb(_) => BPF_PROG_TYPE_SK_SKB,
            Self::SockOps(_) => BPF_PROG_TYPE_SOCK_OPS,
            Self::SchedClassifier(_) => BPF_PROG_TYPE_SCHED_CLS,
            Self::CgroupSkb(_) => BPF_PROG_TYPE_CGROUP_SKB,
            Self::CgroupSysctl(_) => BPF_PROG_TYPE_CGROUP_SYSCTL,
            Self::CgroupSockopt(_) => BPF_PROG_TYPE_CGROUP_SOCKOPT,
            Self::LircMode2(_) => BPF_PROG_TYPE_LIRC_MODE2,
            Self::PerfEvent(_) => BPF_PROG_TYPE_PERF_EVENT,
            Self::RawTracePoint(_) => BPF_PROG_TYPE_RAW_TRACEPOINT,
            Self::Lsm(_) => BPF_PROG_TYPE_LSM,
            Self::BtfTracePoint(_) => BPF_PROG_TYPE_TRACING,
            Self::FEntry(_) => BPF_PROG_TYPE_TRACING,
            Self::FExit(_) => BPF_PROG_TYPE_TRACING,
            Self::Extension(_) => BPF_PROG_TYPE_EXT,
            Self::CgroupSockAddr(_) => BPF_PROG_TYPE_CGROUP_SOCK_ADDR,
            Self::SkLookup(_) => BPF_PROG_TYPE_SK_LOOKUP,
            Self::CgroupSock(_) => BPF_PROG_TYPE_CGROUP_SOCK,
            Self::CgroupDevice(_) => BPF_PROG_TYPE_CGROUP_DEVICE,
        }
    }

    /// Pin the program to the provided path
    pub fn pin<P: AsRef<Path>>(&mut self, path: P) -> Result<(), PinError> {
        match self {
            Self::KProbe(p) => p.pin(path),
            Self::UProbe(p) => p.pin(path),
            Self::TracePoint(p) => p.pin(path),
            Self::SocketFilter(p) => p.pin(path),
            Self::Xdp(p) => p.pin(path),
            Self::SkMsg(p) => p.pin(path),
            Self::SkSkb(p) => p.pin(path),
            Self::SockOps(p) => p.pin(path),
            Self::SchedClassifier(p) => p.pin(path),
            Self::CgroupSkb(p) => p.pin(path),
            Self::CgroupSysctl(p) => p.pin(path),
            Self::CgroupSockopt(p) => p.pin(path),
            Self::LircMode2(p) => p.pin(path),
            Self::PerfEvent(p) => p.pin(path),
            Self::RawTracePoint(p) => p.pin(path),
            Self::Lsm(p) => p.pin(path),
            Self::BtfTracePoint(p) => p.pin(path),
            Self::FEntry(p) => p.pin(path),
            Self::FExit(p) => p.pin(path),
            Self::Extension(p) => p.pin(path),
            Self::CgroupSockAddr(p) => p.pin(path),
            Self::SkLookup(p) => p.pin(path),
            Self::CgroupSock(p) => p.pin(path),
            Self::CgroupDevice(p) => p.pin(path),
        }
    }

    /// Unloads the program from the kernel.
    pub fn unload(self) -> Result<(), ProgramError> {
        match self {
            Self::KProbe(mut p) => p.unload(),
            Self::UProbe(mut p) => p.unload(),
            Self::TracePoint(mut p) => p.unload(),
            Self::SocketFilter(mut p) => p.unload(),
            Self::Xdp(mut p) => p.unload(),
            Self::SkMsg(mut p) => p.unload(),
            Self::SkSkb(mut p) => p.unload(),
            Self::SockOps(mut p) => p.unload(),
            Self::SchedClassifier(mut p) => p.unload(),
            Self::CgroupSkb(mut p) => p.unload(),
            Self::CgroupSysctl(mut p) => p.unload(),
            Self::CgroupSockopt(mut p) => p.unload(),
            Self::LircMode2(mut p) => p.unload(),
            Self::PerfEvent(mut p) => p.unload(),
            Self::RawTracePoint(mut p) => p.unload(),
            Self::Lsm(mut p) => p.unload(),
            Self::BtfTracePoint(mut p) => p.unload(),
            Self::FEntry(mut p) => p.unload(),
            Self::FExit(mut p) => p.unload(),
            Self::Extension(mut p) => p.unload(),
            Self::CgroupSockAddr(mut p) => p.unload(),
            Self::SkLookup(mut p) => p.unload(),
            Self::CgroupSock(mut p) => p.unload(),
            Self::CgroupDevice(mut p) => p.unload(),
        }
    }

    /// Returns the file descriptor of a program.
    ///
    /// Can be used to add a program to a [`crate::maps::ProgramArray`] or attach an [`Extension`] program.
    pub fn fd(&self) -> Result<&ProgramFd, ProgramError> {
        match self {
            Self::KProbe(p) => p.fd(),
            Self::UProbe(p) => p.fd(),
            Self::TracePoint(p) => p.fd(),
            Self::SocketFilter(p) => p.fd(),
            Self::Xdp(p) => p.fd(),
            Self::SkMsg(p) => p.fd(),
            Self::SkSkb(p) => p.fd(),
            Self::SockOps(p) => p.fd(),
            Self::SchedClassifier(p) => p.fd(),
            Self::CgroupSkb(p) => p.fd(),
            Self::CgroupSysctl(p) => p.fd(),
            Self::CgroupSockopt(p) => p.fd(),
            Self::LircMode2(p) => p.fd(),
            Self::PerfEvent(p) => p.fd(),
            Self::RawTracePoint(p) => p.fd(),
            Self::Lsm(p) => p.fd(),
            Self::BtfTracePoint(p) => p.fd(),
            Self::FEntry(p) => p.fd(),
            Self::FExit(p) => p.fd(),
            Self::Extension(p) => p.fd(),
            Self::CgroupSockAddr(p) => p.fd(),
            Self::SkLookup(p) => p.fd(),
            Self::CgroupSock(p) => p.fd(),
            Self::CgroupDevice(p) => p.fd(),
        }
    }

    /// Returns information about a loaded program with the [`ProgramInfo`] structure.
    ///
    /// This information is populated at load time by the kernel and can be used
    /// to get kernel details for a given [`Program`].
    pub fn info(&self) -> Result<ProgramInfo, ProgramError> {
        match self {
            Self::KProbe(p) => p.info(),
            Self::UProbe(p) => p.info(),
            Self::TracePoint(p) => p.info(),
            Self::SocketFilter(p) => p.info(),
            Self::Xdp(p) => p.info(),
            Self::SkMsg(p) => p.info(),
            Self::SkSkb(p) => p.info(),
            Self::SockOps(p) => p.info(),
            Self::SchedClassifier(p) => p.info(),
            Self::CgroupSkb(p) => p.info(),
            Self::CgroupSysctl(p) => p.info(),
            Self::CgroupSockopt(p) => p.info(),
            Self::LircMode2(p) => p.info(),
            Self::PerfEvent(p) => p.info(),
            Self::RawTracePoint(p) => p.info(),
            Self::Lsm(p) => p.info(),
            Self::BtfTracePoint(p) => p.info(),
            Self::FEntry(p) => p.info(),
            Self::FExit(p) => p.info(),
            Self::Extension(p) => p.info(),
            Self::CgroupSockAddr(p) => p.info(),
            Self::SkLookup(p) => p.info(),
            Self::CgroupSock(p) => p.info(),
            Self::CgroupDevice(p) => p.info(),
        }
    }
}

#[derive(Debug)]
pub(crate) struct ProgramData<T: Link> {
    pub(crate) name: Option<String>,
    pub(crate) obj: Option<(obj::Program, obj::Function)>,
    pub(crate) fd: Option<ProgramFd>,
    pub(crate) links: LinkMap<T>,
    pub(crate) expected_attach_type: Option<bpf_attach_type>,
    pub(crate) attach_btf_obj_fd: Option<OwnedFd>,
    pub(crate) attach_btf_id: Option<u32>,
    pub(crate) attach_prog_fd: Option<ProgramFd>,
    pub(crate) btf_fd: Option<Arc<OwnedFd>>,
    pub(crate) verifier_log_level: VerifierLogLevel,
    pub(crate) path: Option<PathBuf>,
    pub(crate) flags: u32,
}

impl<T: Link> ProgramData<T> {
    pub(crate) fn new(
        name: Option<String>,
        obj: (obj::Program, obj::Function),
        btf_fd: Option<Arc<OwnedFd>>,
        verifier_log_level: VerifierLogLevel,
    ) -> Self {
        Self {
            name,
            obj: Some(obj),
            fd: None,
            links: LinkMap::new(),
            expected_attach_type: None,
            attach_btf_obj_fd: None,
            attach_btf_id: None,
            attach_prog_fd: None,
            btf_fd,
            verifier_log_level,
            path: None,
            flags: 0,
        }
    }

    pub(crate) fn from_bpf_prog_info(
        name: Option<String>,
        fd: OwnedFd,
        path: &Path,
        info: bpf_prog_info,
        verifier_log_level: VerifierLogLevel,
    ) -> Result<Self, ProgramError> {
        let attach_btf_id = if info.attach_btf_id > 0 {
            Some(info.attach_btf_id)
        } else {
            None
        };
        let attach_btf_obj_fd = (info.attach_btf_obj_id != 0)
            .then(|| bpf_btf_get_fd_by_id(info.attach_btf_obj_id))
            .transpose()?;

        Ok(Self {
            name,
            obj: None,
            fd: Some(ProgramFd(fd)),
            links: LinkMap::new(),
            expected_attach_type: None,
            attach_btf_obj_fd,
            attach_btf_id,
            attach_prog_fd: None,
            btf_fd: None,
            verifier_log_level,
            path: Some(path.to_path_buf()),
            flags: 0,
        })
    }

    pub(crate) fn from_pinned_path<P: AsRef<Path>>(
        path: P,
        verifier_log_level: VerifierLogLevel,
    ) -> Result<Self, ProgramError> {
        use std::os::unix::ffi::OsStrExt as _;

        // TODO: avoid this unwrap by adding a new error variant.
        let path_string = CString::new(path.as_ref().as_os_str().as_bytes()).unwrap();
        let fd = bpf_get_object(&path_string).map_err(|(_, io_error)| SyscallError {
            call: "bpf_obj_get",
            io_error,
        })?;

        let info = ProgramInfo::new_from_fd(fd.as_fd())?;
        let name = info.name_as_str().map(|s| s.to_string());
        Self::from_bpf_prog_info(name, fd, path.as_ref(), info.0, verifier_log_level)
    }
}

impl<T: Link> ProgramData<T> {
    fn fd(&self) -> Result<&ProgramFd, ProgramError> {
        self.fd.as_ref().ok_or(ProgramError::NotLoaded)
    }

    pub(crate) fn take_link(&mut self, link_id: T::Id) -> Result<T, ProgramError> {
        self.links.forget(link_id)
    }
}

fn unload_program<T: Link>(data: &mut ProgramData<T>) -> Result<(), ProgramError> {
    data.links.remove_all()?;
    data.fd
        .take()
        .ok_or(ProgramError::NotLoaded)
        .map(|ProgramFd { .. }| ())
}

fn pin_program<T: Link, P: AsRef<Path>>(data: &ProgramData<T>, path: P) -> Result<(), PinError> {
    use std::os::unix::ffi::OsStrExt as _;

    let fd = data.fd.as_ref().ok_or(PinError::NoFd {
        name: data
            .name
            .as_deref()
            .unwrap_or("<unknown program>")
            .to_string(),
    })?;
    let path = path.as_ref();
    let path_string =
        CString::new(path.as_os_str().as_bytes()).map_err(|error| PinError::InvalidPinPath {
            path: path.into(),
            error,
        })?;
    bpf_pin_object(fd.as_fd(), &path_string).map_err(|(_, io_error)| SyscallError {
        call: "BPF_OBJ_PIN",
        io_error,
    })?;
    Ok(())
}

fn load_program<T: Link>(
    prog_type: bpf_prog_type,
    data: &mut ProgramData<T>,
) -> Result<(), ProgramError> {
    let ProgramData {
        name,
        obj,
        fd,
        links: _,
        expected_attach_type,
        attach_btf_obj_fd,
        attach_btf_id,
        attach_prog_fd,
        btf_fd,
        verifier_log_level,
        path: _,
        flags,
    } = data;
    if fd.is_some() {
        return Err(ProgramError::AlreadyLoaded);
    }
    if obj.is_none() {
        // This program was loaded from a pin in bpffs
        return Err(ProgramError::AlreadyLoaded);
    }
    let obj = obj.as_ref().unwrap();
    let (
        crate::obj::Program {
            license,
            kernel_version,
            ..
        },
        obj::Function {
            instructions,
            func_info,
            line_info,
            func_info_rec_size,
            line_info_rec_size,
            ..
        },
    ) = obj;

    let target_kernel_version =
        kernel_version.unwrap_or_else(|| KernelVersion::current().unwrap().code());

    let prog_name = if let Some(name) = name {
        let mut name = name.clone();
        if name.len() > 15 {
            name.truncate(15);
        }
        let prog_name = CString::new(name.clone())
            .map_err(|_| ProgramError::InvalidName { name: name.clone() })?;
        Some(prog_name)
    } else {
        None
    };

    let attr = BpfLoadProgramAttrs {
        name: prog_name,
        ty: prog_type,
        insns: instructions,
        license,
        kernel_version: target_kernel_version,
        expected_attach_type: *expected_attach_type,
        prog_btf_fd: btf_fd.as_ref().map(|f| f.as_fd()),
        attach_btf_obj_fd: attach_btf_obj_fd.as_ref().map(|fd| fd.as_fd()),
        attach_btf_id: *attach_btf_id,
        attach_prog_fd: attach_prog_fd.as_ref().map(|fd| fd.as_fd()),
        func_info_rec_size: *func_info_rec_size,
        func_info: func_info.clone(),
        line_info_rec_size: *line_info_rec_size,
        line_info: line_info.clone(),
        flags: *flags,
    };

    let (ret, verifier_log) = retry_with_verifier_logs(10, |logger| {
        bpf_load_program(&attr, logger, *verifier_log_level)
    });

    match ret {
        Ok(prog_fd) => {
            *fd = Some(ProgramFd(prog_fd));
            Ok(())
        }
        Err((_, io_error)) => Err(ProgramError::LoadError {
            io_error,
            verifier_log,
        }),
    }
}

pub(crate) fn query(
    target_fd: BorrowedFd<'_>,
    attach_type: bpf_attach_type,
    query_flags: u32,
    attach_flags: &mut Option<u32>,
) -> Result<Vec<u32>, ProgramError> {
    let mut prog_ids = vec![0u32; 64];
    let mut prog_cnt = prog_ids.len() as u32;

    let mut retries = 0;

    loop {
        match bpf_prog_query(
            target_fd.as_fd().as_raw_fd(),
            attach_type,
            query_flags,
            attach_flags.as_mut(),
            &mut prog_ids,
            &mut prog_cnt,
        ) {
            Ok(_) => {
                prog_ids.resize(prog_cnt as usize, 0);
                return Ok(prog_ids);
            }
            Err((_, io_error)) => {
                if retries == 0 && io_error.raw_os_error() == Some(ENOSPC) {
                    prog_ids.resize(prog_cnt as usize, 0);
                    retries += 1;
                } else {
                    return Err(SyscallError {
                        call: "bpf_prog_query",
                        io_error,
                    }
                    .into());
                }
            }
        }
    }
}

macro_rules! impl_program_unload {
    ($($struct_name:ident),+ $(,)?) => {
        $(
            impl $struct_name {
                /// Unloads the program from the kernel.
                ///
                /// Links will be detached before unloading the program.  Note
                /// that owned links obtained using `take_link()` will not be
                /// detached.
                pub fn unload(&mut self) -> Result<(), ProgramError> {
                    unload_program(&mut self.data)
                }
            }

            impl Drop for $struct_name {
                fn drop(&mut self) {
                    let _ = self.unload();
                }
            }
        )+
    }
}

impl_program_unload!(
    KProbe,
    UProbe,
    TracePoint,
    SocketFilter,
    Xdp,
    SkMsg,
    SkSkb,
    SchedClassifier,
    CgroupSkb,
    CgroupSysctl,
    CgroupSockopt,
    LircMode2,
    PerfEvent,
    Lsm,
    RawTracePoint,
    BtfTracePoint,
    FEntry,
    FExit,
    Extension,
    CgroupSockAddr,
    SkLookup,
    SockOps,
    CgroupSock,
    CgroupDevice,
);

macro_rules! impl_fd {
    ($($struct_name:ident),+ $(,)?) => {
        $(
            impl $struct_name {
                /// Returns the file descriptor of this Program.
                pub fn fd(&self) -> Result<&ProgramFd, ProgramError> {
                    self.data.fd()
                }
            }
        )+
    }
}

impl_fd!(
    KProbe,
    UProbe,
    TracePoint,
    SocketFilter,
    Xdp,
    SkMsg,
    SkSkb,
    SchedClassifier,
    CgroupSkb,
    CgroupSysctl,
    CgroupSockopt,
    LircMode2,
    PerfEvent,
    Lsm,
    RawTracePoint,
    BtfTracePoint,
    FEntry,
    FExit,
    Extension,
    CgroupSockAddr,
    SkLookup,
    SockOps,
    CgroupSock,
    CgroupDevice,
);

macro_rules! impl_program_pin{
    ($($struct_name:ident),+ $(,)?) => {
        $(
            impl $struct_name {
                /// Pins the program to a BPF filesystem.
                ///
                /// When a BPF object is pinned to a BPF filesystem it will remain loaded after
                /// Aya has unloaded the program.
                /// To remove the program, the file on the BPF filesystem must be removed.
                /// Any directories in the the path provided should have been created by the caller.
                pub fn pin<P: AsRef<Path>>(&mut self, path: P) -> Result<(), PinError> {
                    self.data.path = Some(path.as_ref().to_path_buf());
                    pin_program(&self.data, path)
                }

                /// Removes the pinned link from the filesystem.
                pub fn unpin(mut self) -> Result<(), io::Error> {
                    if let Some(path) = self.data.path.take() {
                        std::fs::remove_file(path)?;
                    }
                    Ok(())
                }
            }
        )+
    }
}

impl_program_pin!(
    KProbe,
    UProbe,
    TracePoint,
    SocketFilter,
    Xdp,
    SkMsg,
    SkSkb,
    SchedClassifier,
    CgroupSkb,
    CgroupSysctl,
    CgroupSockopt,
    LircMode2,
    PerfEvent,
    Lsm,
    RawTracePoint,
    BtfTracePoint,
    FEntry,
    FExit,
    Extension,
    CgroupSockAddr,
    SkLookup,
    SockOps,
    CgroupSock,
    CgroupDevice,
);

macro_rules! impl_from_pin {
    ($($struct_name:ident),+ $(,)?) => {
        $(
            impl $struct_name {
                /// Creates a program from a pinned entry on a bpffs.
                ///
                /// Existing links will not be populated. To work with existing links you should use [`crate::programs::links::PinnedLink`].
                ///
                /// On drop, any managed links are detached and the program is unloaded. This will not result in
                /// the program being unloaded from the kernel if it is still pinned.
                pub fn from_pin<P: AsRef<Path>>(path: P) -> Result<Self, ProgramError> {
                    let data = ProgramData::from_pinned_path(path, VerifierLogLevel::default())?;
                    Ok(Self { data })
                }
            }
        )+
    }
}

// Use impl_from_pin if the program doesn't require additional data
impl_from_pin!(
    TracePoint,
    SocketFilter,
    SkMsg,
    CgroupSysctl,
    LircMode2,
    PerfEvent,
    Lsm,
    RawTracePoint,
    BtfTracePoint,
    FEntry,
    FExit,
    Extension,
    SkLookup,
    SockOps,
    CgroupDevice,
);

macro_rules! impl_try_from_program {
    ($($ty:ident),+ $(,)?) => {
        $(
            impl<'a> TryFrom<&'a Program> for &'a $ty {
                type Error = ProgramError;

                fn try_from(program: &'a Program) -> Result<&'a $ty, ProgramError> {
                    match program {
                        Program::$ty(p) => Ok(p),
                        _ => Err(ProgramError::UnexpectedProgramType),
                    }
                }
            }

            impl<'a> TryFrom<&'a mut Program> for &'a mut $ty {
                type Error = ProgramError;

                fn try_from(program: &'a mut Program) -> Result<&'a mut $ty, ProgramError> {
                    match program {
                        Program::$ty(p) => Ok(p),
                        _ => Err(ProgramError::UnexpectedProgramType),
                    }
                }
            }
        )+
    }
}

impl_try_from_program!(
    KProbe,
    UProbe,
    TracePoint,
    SocketFilter,
    Xdp,
    SkMsg,
    SkSkb,
    SockOps,
    SchedClassifier,
    CgroupSkb,
    CgroupSysctl,
    CgroupSockopt,
    LircMode2,
    PerfEvent,
    Lsm,
    RawTracePoint,
    BtfTracePoint,
    FEntry,
    FExit,
    Extension,
    CgroupSockAddr,
    SkLookup,
    CgroupSock,
    CgroupDevice,
);

/// Returns information about a loaded program with the [`ProgramInfo`] structure.
///
/// This information is populated at load time by the kernel and can be used
/// to correlate a given [`Program`] to it's corresponding [`ProgramInfo`]
/// metadata.
macro_rules! impl_info {
    ($($struct_name:ident),+ $(,)?) => {
        $(
            impl $struct_name {
                /// Returns the file descriptor of this Program.
                pub fn info(&self) -> Result<ProgramInfo, ProgramError> {
                    let ProgramFd(fd) = self.fd()?;

                    ProgramInfo::new_from_fd(fd.as_fd())
                }
            }
        )+
    }
}

impl_info!(
    KProbe,
    UProbe,
    TracePoint,
    SocketFilter,
    Xdp,
    SkMsg,
    SkSkb,
    SchedClassifier,
    CgroupSkb,
    CgroupSysctl,
    CgroupSockopt,
    LircMode2,
    PerfEvent,
    Lsm,
    RawTracePoint,
    BtfTracePoint,
    FEntry,
    FExit,
    Extension,
    CgroupSockAddr,
    SkLookup,
    SockOps,
    CgroupSock,
    CgroupDevice,
);

/// Provides information about a loaded program, like name, id and statistics
#[derive(Debug)]
pub struct ProgramInfo(bpf_prog_info);

impl ProgramInfo {
    fn new_from_fd(fd: BorrowedFd<'_>) -> Result<Self, ProgramError> {
        let info = bpf_prog_get_info_by_fd(fd, &mut [])?;
        Ok(Self(info))
    }

    /// The name of the program as was provided when it was load. This is limited to 16 bytes
    pub fn name(&self) -> &[u8] {
        let length = self
            .0
            .name
            .iter()
            .rposition(|ch| *ch != 0)
            .map(|pos| pos + 1)
            .unwrap_or(0);

        // The name field is defined as [std::os::raw::c_char; 16]. c_char may be signed or
        // unsigned depending on the platform; that's why we're using from_raw_parts here
        unsafe { std::slice::from_raw_parts(self.0.name.as_ptr() as *const _, length) }
    }

    /// The name of the program as a &str. If the name was not valid unicode, None is returned.
    pub fn name_as_str(&self) -> Option<&str> {
        std::str::from_utf8(self.name()).ok()
    }

    /// The id for this program. Each program has a unique id.
    pub fn id(&self) -> u32 {
        self.0.id
    }

    /// The program tag.
    ///
    /// The program tag is a SHA sum of the program's instructions which be used as an alternative to
    /// [`Self::id()`]". A program's id can vary every time it's loaded or unloaded, but the tag
    /// will remain the same.
    pub fn tag(&self) -> u64 {
        u64::from_be_bytes(self.0.tag)
    }

    /// The program type as defined by the linux kernel enum
    /// [`bpf_prog_type`](https://elixir.bootlin.com/linux/v6.4.4/source/include/uapi/linux/bpf.h#L948).
    pub fn program_type(&self) -> u32 {
        self.0.type_
    }

    /// Returns true if the program is defined with a GPL-compatible license.
    pub fn gpl_compatible(&self) -> bool {
        self.0.gpl_compatible() != 0
    }

    /// The ids of the maps used by the program.
    pub fn map_ids(&self) -> Result<Vec<u32>, ProgramError> {
        let ProgramFd(fd) = self.fd()?;
        let mut map_ids = vec![0u32; self.0.nr_map_ids as usize];

        bpf_prog_get_info_by_fd(fd.as_fd(), &mut map_ids)?;

        Ok(map_ids)
    }

    /// The btf id for the program.
    pub fn btf_id(&self) -> Option<NonZeroU32> {
        NonZeroU32::new(self.0.btf_id)
    }

    /// The size in bytes of the program's translated eBPF bytecode, which is
    /// the bytecode after it has been passed though the verifier where it was
    /// possibly modified by the kernel.
    pub fn size_translated(&self) -> u32 {
        self.0.xlated_prog_len
    }

    /// The size in bytes of the program's JIT-compiled machine code.
    pub fn size_jitted(&self) -> u32 {
        self.0.jited_prog_len
    }

    /// How much memory in bytes has been allocated and locked for the program.
    pub fn memory_locked(&self) -> Result<u32, ProgramError> {
        get_fdinfo(self.fd()?.as_fd(), "memlock")
    }

    /// The number of verified instructions in the program.
    ///
    /// This may be less than the total number of instructions in the compiled
    /// program due to dead code elimination in the verifier.
    pub fn verified_instruction_count(&self) -> u32 {
        self.0.verified_insns
    }

    /// The time the program was loaded.
    pub fn loaded_at(&self) -> SystemTime {
        boot_time() + Duration::from_nanos(self.0.load_time)
    }

    /// Returns a file descriptor referencing the program.
    ///
    /// The returned file descriptor can be closed at any time and doing so does
    /// not influence the life cycle of the program.
    pub fn fd(&self) -> Result<ProgramFd, ProgramError> {
        let Self(info) = self;
        let fd = bpf_prog_get_fd_by_id(info.id)?;
        Ok(ProgramFd(fd))
    }

    /// Loads a program from a pinned path in bpffs.
    pub fn from_pin<P: AsRef<Path>>(path: P) -> Result<Self, ProgramError> {
        use std::os::unix::ffi::OsStrExt as _;

        // TODO: avoid this unwrap by adding a new error variant.
        let path_string = CString::new(path.as_ref().as_os_str().as_bytes()).unwrap();
        let fd = bpf_get_object(&path_string).map_err(|(_, io_error)| SyscallError {
            call: "BPF_OBJ_GET",
            io_error,
        })?;

        let info = bpf_prog_get_info_by_fd(fd.as_fd(), &mut [])?;
        Ok(Self(info))
    }
}

/// Returns an iterator over all loaded bpf programs.
///
/// This differs from [`crate::Bpf::programs`] since it will return all programs
/// listed on the host system and not only programs a specific [`crate::Bpf`] instance.
///
/// # Example
/// ```
/// # use aya::programs::loaded_programs;
///
/// for p in loaded_programs() {
///     match p {
///         Ok(program) => println!("{}", String::from_utf8_lossy(program.name())),
///         Err(e) => println!("Error iterating programs: {:?}", e),
///     }
/// }
/// ```
///
/// # Errors
///
/// Returns [`ProgramError::SyscallError`] if any of the syscalls required to either get
/// next program id, get the program fd, or the [`ProgramInfo`] fail. In cases where
/// iteration can't be performed, for example the caller does not have the necessary privileges,
/// a single item will be yielded containing the error that occurred.
pub fn loaded_programs() -> impl Iterator<Item = Result<ProgramInfo, ProgramError>> {
    iter_prog_ids()
        .map(|id| {
            let id = id?;
            bpf_prog_get_fd_by_id(id)
        })
        .map(|fd| {
            let fd = fd?;
            bpf_prog_get_info_by_fd(fd.as_fd(), &mut [])
        })
        .map(|result| result.map(ProgramInfo).map_err(Into::into))
}

// TODO(https://github.com/aya-rs/aya/issues/645): this API is currently used in tests. Stabilize
// and remove doc(hidden).
#[doc(hidden)]
pub fn loaded_links() -> impl Iterator<Item = Result<bpf_link_info, ProgramError>> {
    iter_link_ids()
        .map(|id| {
            let id = id?;
            bpf_link_get_fd_by_id(id)
        })
        .map(|fd| {
            let fd = fd?;
            bpf_link_get_info_by_fd(fd.as_fd())
        })
        .map(|result| result.map_err(Into::into))
}
