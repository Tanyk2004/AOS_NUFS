use fuser::{
    FileAttr, FileType, Filesystem, MountOption, ReplyAttr, ReplyData, ReplyDirectory, ReplyEntry,
    ReplyOpen, ReplyWrite, Request, TimeOrNow,
};

use ssh2::{Agent, Session, Sftp};

use libc::{
    EACCES, EEXIST, EINVAL, EIO, ENOENT, ENOTDIR, O_ACCMODE, O_RDONLY, O_RDWR, O_WRONLY, write,
};

use core::str;
use std::{
    collections::HashMap,
    env::ArgsOs,
    ffi::OsStr,
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, SystemTime},
};

const TTL: Duration = Duration::from_secs(1); // 1 second
const ROOT_INODE: u64 = 1;
const CACHE_PATH: &str = "/var/tmp/tulfs_cache";
const PRIVATE_KEY: &str = "/home/tanay24/.ssh/networked_fs";

struct OpenEntry {
    file: File,
    ino: u64,
    flags: u32,
    dirty: bool,
}

#[derive(Default)]
struct State {
    inode_to_path: HashMap<u64, PathBuf>,
    path_to_inode: HashMap<PathBuf, u64>,

    next_fh: u64,                        // Next available file handle
    open_files: HashMap<u64, OpenEntry>, // Map of file handle to OpenEntry
}

struct TULFS {
    user: String,
    host: String,
    sftp: Sftp,
    server_hash: String, // Hash of the server hostname so that multiple instances don't conflict
    backing_root: PathBuf, // Remote backing root directory
    st: Arc<Mutex<State>>, // Shared state locked with a mutex
}

impl TULFS {
    fn new(hostname: String, backing_root: PathBuf) -> Self {
        let st = Arc::new(Mutex::new(State::default()));
        st.lock().unwrap().next_fh = 1; // Start file handles at 1
        let hostname_parts: Vec<String> = hostname.splitn(2, '@').map(|s| s.to_string()).collect();
        if hostname_parts.len() != 2 {
            eprintln!("[ERROR] Hostname must be in the format user@host");
            std::process::exit(1);
        }
        let user = hostname_parts[0].clone();
        let host = hostname_parts[1].clone();
        // connect to the server here using sftp
        let tcp =
            std::net::TcpStream::connect((host.clone(), 22)).expect("Could not connect to server");
        // create ssh session
        let mut session = Session::new().expect("Could not create SSH session");
        session.set_tcp_stream(tcp);
        session
            .handshake()
            .expect("Could not complete SSH handshake");
        session
            .userauth_pubkey_file(&user, None, &Path::new(PRIVATE_KEY), None)
            .expect("Could not authenticate");

        // check if the backing directory is actually a directory on the server using sftp
        let sftp = session.sftp().expect("Could not create SFTP session");
        let backing_metadata = sftp.stat(Path::new(backing_root.to_str().unwrap()));
        if backing_metadata.is_err() || !backing_metadata.unwrap().is_dir() {
            eprintln!("[ERROR] Backing directory is not a valid directory on the server");
            std::process::exit(1);
        }

        // create local cache folder /var/tmp/tulfs_cache/<server_hash> if it doesn't exist
        let server_hash = format!("{:x}", md5::compute(&hostname));
        let cache_dir = PathBuf::from(format!("{}/{}", CACHE_PATH, server_hash));
        if !cache_dir.exists() {
            fs::create_dir_all(&cache_dir).expect("Could not create cache directory");
        }

        TULFS {
            user,
            host,
            sftp,
            server_hash,
            backing_root,
            st,
        }
    }

    fn is_remote_dir(&self, path: &Path) -> bool {
        let metadata = self.sftp.stat(path);
        metadata.map(|m| m.is_dir()).unwrap_or(false)
    }

    /**
     * Adds /var/tmp/tulfs_cache/<server_hash> as prefix to path
     */
    fn get_local_abs_path(&self, path: &Path) -> PathBuf {
        let mut local_path = PathBuf::from(CACHE_PATH);
        local_path.push(&self.server_hash);
        for component in path.components() {
            local_path.push(component.as_os_str());
        }
        local_path
    }

    // Adds backing_root as prefix to path
    fn get_remote_abs_path(&self, rel: &Path) -> PathBuf {
        println!(
            "get_remote_abs_path: path = {:?} Backing Root: {:?}",
            rel, self.backing_root
        );
        let mut remote_path = self.backing_root.clone();

        // append each component of path to remote_path
        let str_path = rel.to_str().unwrap_or("");
        str_path.strip_prefix("/").map(|s| remote_path.push(s));
        for component in rel.components() {
            remote_path.push(component.as_os_str());
        }

        println!("Remote absolute path: {:?}", remote_path);
        remote_path
    }

    fn ensure_root(&self) {
        let mut st = self.st.lock().unwrap();
        if !st.inode_to_path.contains_key(&ROOT_INODE) {
            st.inode_to_path.insert(ROOT_INODE, PathBuf::from("/"));
            st.path_to_inode.insert(PathBuf::from("/"), ROOT_INODE);
        }
    }

    fn root_attr(&self) -> FileAttr {
        let uid = unsafe { libc::getuid() } as u32;
        let gid = unsafe { libc::getgid() } as u32;
        FileAttr {
            ino: ROOT_INODE,
            size: 0,
            blocks: 0,
            atime: SystemTime::now(),
            mtime: SystemTime::now(),
            ctime: SystemTime::now(),
            crtime: SystemTime::now(),
            kind: FileType::Directory,
            perm: 0o755,
            nlink: 2,
            uid,
            gid,
            rdev: 0,
            blksize: 512,
            flags: 0,
        }
    }

    /**
     *  Takes in relative path and returns inode number.
     */
    fn inode_for_path(&self, rel_path: &Path) -> u64 {
        println!("inode_for_path: path = {:?}", rel_path);

        let rel_path = rel_path.strip_prefix("/").unwrap_or(&rel_path).to_path_buf();
        let resolved_path = rel_path.to_str();
        let binding = PathBuf::from("/");
        let canonical_root = binding.to_str();
        println!("resolved_path: {:?}, canonical_root: {:?}", resolved_path, canonical_root);
        if resolved_path == canonical_root {
            return ROOT_INODE;
        }
        if let Some(&ino) = self.st.lock().unwrap().path_to_inode.get(&rel_path) {
            println!("Found inode: {:?} for path: {:?}", ino, rel_path);
            return ino;
        }

        // Generate a new inode number based on the hash of the path
        let d = md5::compute(resolved_path.unwrap().as_bytes()); // I don't want to bother with inode number collisions 
        let ino = u64::from_be_bytes([d[0], d[1], d[2], d[3], d[4], d[5], d[6], d[7]]);
        let mut st = self.st.lock().unwrap();
        println!("Inserting mapping: ino = {:?} rel_path: {:?}", ino, rel_path);
        st.path_to_inode.insert(PathBuf::from(&rel_path), ino);
        st.inode_to_path.insert(ino, PathBuf::from(&rel_path));
        ino
    }

    fn path_for_inode(&self, ino: u64) -> Option<PathBuf> {
        if ino == ROOT_INODE {
            return Some(PathBuf::from("/"));
        }
        let st = self.st.lock().unwrap();
        println!("Does inode_to_path contain key {}? {}", ino, st.inode_to_path.contains_key(&ino));
        if !st.inode_to_path.contains_key(&ino) {
            return None;
        }
        st.inode_to_path.get(&ino).cloned()
    }

    /**
     * Get file attributes from the remote server using `sftp.stat`
     *
     * Parameters:
     * - rel: PathBuf - relative path from the backing root
     * - ino: u64 - inode number
     * Returns:
     * - Result<FileAttr, libc::c_int> - Ok(FileAttr) if successful
     */
    fn attr_from_remote(&self, rel: PathBuf, ino: u64) -> Result<FileAttr, libc::c_int> {
        println!("attr_from_remote: rel = {:?}", rel);
        let full_path = self.get_remote_abs_path(&rel);
        println!("attr_from_remote: full_path = {:?}", full_path);
        let stat = self.sftp.stat(&full_path).map_err(|_| ENOENT)?;
        let now = SystemTime::now();
        let uid = stat
            .uid
            .map(|u| u as u32)
            .unwrap_or_else(|| unsafe { libc::getuid() as u32 });
        let gid = stat
            .gid
            .map(|g| g as u32)
            .unwrap_or_else(|| unsafe { libc::getgid() as u32 });

        let is_dir = self.is_remote_dir(&full_path);
        let kind = if is_dir {
            FileType::Directory
        } else {
            FileType::RegularFile
        };
        let perm = stat.perm.unwrap_or(if is_dir { 0o755 } else { 0o644 }) as u16;
        let size = if is_dir { 0 } else { stat.size.unwrap_or(0) };

        let atime = stat
            .atime
            .map(|s| SystemTime::UNIX_EPOCH + Duration::from_secs(s as u64))
            .unwrap_or(now);
        let mtime = stat
            .mtime
            .map(|s| SystemTime::UNIX_EPOCH + Duration::from_secs(s as u64))
            .unwrap_or(now);

        Ok(FileAttr {
            ino,
            size,
            blocks: 0,
            atime,
            mtime,
            ctime: mtime,
            crtime: mtime,
            kind,
            perm,
            nlink: if is_dir { 2 } else { 1 },
            uid,
            gid,
            rdev: 0,
            blksize: 512,
            flags: 0,
        })
    }

    fn fetch_file_from_remote(&self, path: &Path) -> Result<File, libc::c_int> {
        let local_path = self.get_local_abs_path(&path);
        let remote_path = self.get_remote_abs_path(&path);
        println!("Fetching file from remote server: {:?}", remote_path);
        let mut remote_file = match self.sftp.open(&remote_path) {
            Ok(f) => f,
            Err(_) => {
                eprintln!("File not found on remote server: {:?}", remote_path);
                return Err(ENOENT);
            }
        };
        let mut local_file = match OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&local_path)
        {
            Ok(f) => f,
            Err(_) => {
                eprintln!("Failed to open local file: {:?}", local_path);
                return Err(EIO);
            }
        };

        println!("Copying file to local cache: {:?}", local_path);
        if let Err(_) = std::io::copy(&mut remote_file, &mut local_file) {
            eprintln!("Failed to copy file to local cache: {:?}", local_path);
            return Err(EIO);
        }

        // Ensure data is flushed to disk
        if let Err(_) = local_file.flush() {
            eprintln!("Failed to flush local file: {:?}", local_path);
            return Err(EIO);
        }

        Ok(local_file)
    }

    fn copy_from_local_to_remote(
        &self,
        mut local_file: File,
        remote_path: &Path,
    ) -> Result<(), libc::c_int> {
        // Rewind local file to start
        if let Err(_) = local_file.seek(SeekFrom::Start(0)) {
            eprintln!("Failed to seek local file to start");
            return Err(EIO);
        }

        // Open remote file for writing
        let mut remote_file = match self.sftp.open_mode(
            &remote_path,
            ssh2::OpenFlags::WRITE | ssh2::OpenFlags::CREATE | ssh2::OpenFlags::TRUNCATE,
            0o644,
            ssh2::OpenType::File,
        ) {
            Ok(f) => f,
            Err(_) => {
                eprintln!("Failed to open remote file: {:?}", remote_path);
                return Err(EIO);
            }
        };

        // Copy data from local file to remote file
        let mut buffer = Vec::new();
        if let Err(_) = local_file.read_to_end(&mut buffer) {
            eprintln!("Failed to read local file");
            return Err(EIO);
        }

        println!("Buffer Contents: {:?}", buffer);

        if let Err(_) = remote_file.write_all(&buffer) {
            eprintln!("Failed to write to remote file: {:?}", remote_path);
            return Err(EIO);
        }

        Ok(())
    }

    fn flush_dirty_files(&self) {
        let mut st = self.st.lock().unwrap();
        for (fh, entry) in st.open_files.iter_mut() {
            if entry.dirty {
                // flush to remote server
                let _ino = entry.ino;
                let path = match self.path_for_inode(_ino) {
                    Some(p) => p,
                    None => {
                        eprintln!("Could not find path for inode {}", _ino);
                        continue;
                    }
                };
                let remote_path = self.get_remote_abs_path(&path);
                let local_path = self.get_local_abs_path(&path);
                println!("Flushing dirty file to remote server: {:?}", remote_path);
                let local_file = match OpenOptions::new().read(true).open(&local_path) {
                    Ok(f) => f,
                    Err(_) => {
                        eprintln!("Failed to open local file: {:?}", local_path);
                        continue;
                    }
                };
                if let Err(e) = self.copy_from_local_to_remote(local_file, &remote_path) {
                    eprintln!("Failed to copy file to remote server: {:?}", remote_path);
                    continue;
                }
            }
        }
    }
}

impl Filesystem for TULFS {
    fn init(
        &mut self,
        _req: &Request<'_>,
        _config: &mut fuser::KernelConfig,
    ) -> Result<(), libc::c_int> {
        print!("init\n");
        self.ensure_root();
        Ok(())
    }

    fn getattr(&mut self, _req: &Request<'_>, ino: u64, reply: ReplyAttr) {
        println!("getattr");
        println!("ino: {}", ino);
        if ino == ROOT_INODE {
            reply.attr(&TTL, &self.root_attr());
            return;
        } else {
            let Some(path) = self.path_for_inode(ino) else {
                reply.error(ENOENT);
                return;
            };

            println!("Path for inode {}: {:?}", ino, path);
            let rel = path.strip_prefix("/").unwrap_or(&path);
            match self.attr_from_remote(rel.to_path_buf(), ino) {
                Ok(attr) => reply.attr(&TTL, &attr),
                Err(e) => reply.error(e),
            }
        }
    }

    fn lookup(&mut self, _req: &Request<'_>, parent: u64, name: &OsStr, reply: ReplyEntry) {
        print!("lookup\n");
        println!("parent: {}, name: {:?}", parent, name);

        // check if parent inode exists
        if !self.path_for_inode(parent).is_some() {
            reply.error(ENOENT);
            return;
        }

        let parent_path = match self.path_for_inode(parent) {
            // Get parent path from inode
            Some(p) => p,
            None => {
                reply.error(ENOENT); // Orphaned file? Something is fs wrong
                return;
            }
        };

        // check if file is open in open_files
        let child_path = parent_path.join(name);
        let ino = self.inode_for_path(&child_path);

        println!("Child path: {:?}", child_path);
        if let Some(attr) = self
            .attr_from_remote(
                child_path
                    .strip_prefix("/")
                    .unwrap_or(&child_path)
                    .to_path_buf(),
                ino,
            )
            .ok()
        {
            reply.entry(&TTL, &attr, 0); // We are not reusing inode numbers keep generation to 0 for now
        } else {
            println!("File not found on remote server");
            reply.error(ENOENT);
        }
    }

    fn open(&mut self, _req: &Request<'_>, _ino: u64, _flags: i32, reply: ReplyOpen) {
        print!("open\n");
        println!("ino: {}, flags: {}", _ino, _flags);

        // check if inode exists
        if !self.path_for_inode(_ino).is_some() {
            reply.error(ENOENT);
            return;
        }

        let Some(path) = self.path_for_inode(_ino) else {
            reply.error(ENOENT);
            return;
        };
        println!("Path for inode {}: {:?}", _ino, path);

        // if the first character of the path is '/', remove it
        let path = path.strip_prefix("/").unwrap_or(&path).to_path_buf();

        // fetch file from remote server if it doesn't exist in local cache
        let local_path = self.get_local_abs_path(&path);
        println!("Local path: {:?}", local_path);
        let mut _fh = 0;
        let mut local_flags = _flags as u32;
        if !local_path.exists() {
            std::fs::create_dir_all(local_path.parent().unwrap()).unwrap();
            let res = self.fetch_file_from_remote(&path);
            if let Err(e) = res {
                reply.error(e);
                return;
            }
            let _local_file = res.unwrap();

            // add file to open_files
            let mut st = self.st.lock().unwrap();
            let open_entry: OpenEntry = OpenEntry {
                file: _local_file,
                flags: local_flags,
                dirty: false,
                ino: _ino,
            };
            _fh = st.next_fh;
            st.open_files.insert(_fh, open_entry);
            st.next_fh += 1;
            drop(st);

            println!("Opened file {:?} with fh {} as new", local_path, _fh);
        } else {
            let accmode = _flags & O_ACCMODE;
            let mut write_access = accmode == O_WRONLY || accmode == O_RDWR;
            // check if _ino is already opened with incompatible flags
            let st = self.st.lock().unwrap();
            if let Some(existing_entry) = st.open_files.get(&_ino) {
                if existing_entry.dirty {
                    write_access = false;
                }
            }
            drop(st);
            // if write access is false, remove write flags from local_flags
            if !write_access {
                local_flags &= !(O_WRONLY as u32);
                local_flags &= !(O_RDWR as u32);
                local_flags |= O_RDONLY as u32;
            }

            let local_file = OpenOptions::new()
                .read(true)
                .write(write_access)
                .open(&local_path);
            if local_file.is_err() {
                reply.error(EIO);
                return;
            }
            let local_file = local_file.unwrap();

            // add file to open_files
            let mut st = self.st.lock().unwrap();
            let open_entry: OpenEntry = OpenEntry {
                file: local_file,
                flags: local_flags,
                ino: _ino,
                dirty: false,
            };
            _fh = st.next_fh;
            st.open_files.insert(_fh, open_entry);
            st.next_fh += 1;
            drop(st);
            println!("Opened file {:?} with fh {} as read only", local_path, _fh);
        }
        reply.opened(_fh, local_flags);
    }

    fn write(
        &mut self,
        _req: &Request<'_>,
        ino: u64,
        fh: u64,
        offset: i64,
        data: &[u8],
        write_flags: u32,
        flags: i32,
        lock_owner: Option<u64>,
        reply: ReplyWrite,
    ) {
        print!("write\n");
        println!(
            "ino: {}, fh: {}, offset: {}, size: {}, write_flags: {}, flags: {}, lock_owner: {:?}",
            ino,
            fh,
            offset,
            data.len(),
            write_flags,
            flags,
            lock_owner
        );

        let mut st = self.st.lock().unwrap();
        let open_entry = match st.open_files.get_mut(&fh) {
            Some(entry) => entry,
            None => {
                reply.error(EINVAL);
                return;
            }
        };

        // Check if the file was opened with write permissions
        let accmode = open_entry.flags & O_ACCMODE as u32;
        if accmode != O_WRONLY as u32 && accmode != O_RDWR as u32 {
            reply.error(EACCES);
            return;
        }

        // Seek to the specified offset
        if let Err(_) = open_entry.file.seek(SeekFrom::Start(offset as u64)) {
            reply.error(EIO);
            return;
        }

        println!("Writing {} bytes at offset {}", data.len(), offset);
        println!("Data Contents: {:?}", data);

        // Write the data
        match open_entry.file.write(data) {
            Ok(bytes_written) => {
                open_entry.dirty = true; // Mark file as dirty
                reply.written(bytes_written as u32);
            }
            Err(_) => {
                reply.error(EIO);
            }
        }
    }

    fn read(
        &mut self,
        _req: &Request<'_>,
        ino: u64,
        fh: u64,
        offset: i64,
        size: u32,
        flags: i32,
        lock_owner: Option<u64>,
        reply: ReplyData,
    ) {
        println!("read");
        println!(
            "ino: {}, fh: {}, offset: {}, size: {}, flags: {}, lock_owner: {:?}",
            ino, fh, offset, size, flags, lock_owner
        );
        let mut file = {
            let st = self.st.lock().unwrap();
            match st.open_files.get(&fh) {
                Some(entry) => entry.file.try_clone().unwrap(),
                None => {
                    reply.error(EINVAL);
                    return;
                }
            }
        };

        // Seek to the specified offset
        if let Err(_) = file.seek(SeekFrom::Start(offset as u64)) {
            reply.error(EIO);
            return;
        }

        let mut buffer = vec![0; size as usize];
        match file.read(&mut buffer) {
            Ok(bytes_read) => {
                reply.data(&buffer[..bytes_read]);
            }
            Err(_) => {
                reply.error(EIO);
            }
        }
    }

    fn flush(
        &mut self,
        _req: &Request<'_>,
        ino: u64,
        fh: u64,
        lock_owner: u64,
        reply: fuser::ReplyEmpty,
    ) {
        print!("flush\n");
        println!("ino: {}, fh: {}, lock_owner: {}", ino, fh, lock_owner);

        // Snapshot needed info under the lock without holding a mutable borrow across IO
        let (is_dirty, entry_ino) = {
            let st = self.st.lock().unwrap();
            match st.open_files.get(&fh) {
                Some(entry) => (entry.dirty, entry.ino),
                None => {
                    reply.error(EINVAL);
                    return;
                }
            }
        };

        if !is_dirty {
            reply.ok();
            return;
        }

        let path = match self.path_for_inode(entry_ino) {
            Some(p) => p,
            None => {
                reply.error(ENOENT);
                return;
            }
        };
        // reaching
        println!("Path from inode {:?}", path);
        let path = path.strip_prefix("/").unwrap_or(&path).to_path_buf();
        let remote_path = self.get_remote_abs_path(&path);
        let local_path = self.get_local_abs_path(&path);
        println!("Flushing dirty file to remote server: {:?}", remote_path);
        let local_file = match OpenOptions::new().read(true).open(&local_path) {
            Ok(f) => f,
            Err(_) => {
                eprintln!("Failed to open local file: {:?}", local_path);
                reply.error(EIO);
                return;
            }
        };
        let res = self.copy_from_local_to_remote(local_file, remote_path.as_path());
        if let Err(e) = res {
            eprintln!("Failed to copy file to remote server: {:?}", remote_path);
            reply.error(e);
            return;
        }

        // Mark file as clean after successful flush
        {
            let mut st = self.st.lock().unwrap();
            if let Some(entry) = st.open_files.get_mut(&fh) {
                entry.dirty = false;
            }
        }

        reply.ok();
    }

    fn release(
        &mut self,
        _req: &Request<'_>,
        _ino: u64,
        _fh: u64,
        _flags: i32,
        _lock_owner: Option<u64>,
        _flush: bool,
        reply: fuser::ReplyEmpty,
    ) {
        print!("release\n");
        println!("ino: {}, fh: {}", _ino, _fh);

        let (is_dirty, entry_ino) = {
            let st = self.st.lock().unwrap();
            match st.open_files.get(&_fh) {
                Some(entry) => (entry.dirty, entry.ino),
                None => {
                    reply.error(EINVAL);
                    return;
                }
            }
        };
        println!("is_dirty: {}, entry_ino: {}", is_dirty, entry_ino);
        let path = match self.path_for_inode(entry_ino) {
            Some(p) => p,
            None => {
                reply.error(ENOENT);
                return;
            }
        };

        println!("Path from inode {:?}", path);
        let path  = path.strip_prefix("/").unwrap_or(&path).to_path_buf();
        if is_dirty {
            let path = path.strip_prefix("/").unwrap_or(&path).to_path_buf();
            let remote_path = self.get_remote_abs_path(&path);
            let local_path = self.get_local_abs_path(&path);
            println!("Flushing dirty file to remote server: {:?}", remote_path);
            let local_file = match OpenOptions::new().read(true).open(&local_path) {
                Ok(f) => f,
                Err(_) => {
                    eprintln!("Failed to open local file: {:?}", local_path);
                    reply.error(EIO);
                    return;
                }
            };
            let res = self.copy_from_local_to_remote(local_file, remote_path.as_path());
            if let Err(e) = res {
                eprintln!("Failed to copy file to remote server: {:?}", remote_path);
                reply.error(e);
                return;
            }
        }

        let mut st = self.st.lock().unwrap();
        st.open_files.remove(&_fh);

        // remove mappings and cached file if no other open files with same inode
        let still_open = st.open_files.values().any(|entry| entry.ino == entry_ino);
        if still_open {
            drop(st);
            reply.ok();
            return;
        }
        println!("Removing inode to path mapping for inode {}", entry_ino);
        st.inode_to_path.remove(&_ino);
        println!("Removing path to inode mapping for path {:?}", path);
        st.path_to_inode.remove(&path);
        drop(st);

        // delete the local cached file
        let local_path = self.get_local_abs_path(&path);
        if local_path.exists() {
            if let Err(e) = fs::remove_file(&local_path) {
                eprintln!("Failed to delete local cached file: {:?}", e);
                // Not a critical error, so we don't return here
            }
        }
        println!("Deleted local cached file: {:?}", local_path);

        reply.ok();
    }
}

// ! ISSUE: Right now if I run the test program twice without shutting down the fuse client then 
// ! I get an error where inode_to_path doesn't contain the inode even though it should.

fn extract_hostname_and_path(backing: &str) -> Option<(&str, &str)> {
    if (!backing.contains(':')) {
        return None;
    }
    let parts: Vec<&str> = backing.splitn(2, ':').collect();
    if parts.len() == 2 {
        Some((parts[0], parts[1]))
    } else {
        None
    }
}

fn main() {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    println!("Args {:?}", args);
    if (args.len() != 2) {
        eprintln!("Usage: client <mountpoint> <user@host:backing_directory>");
        std::process::exit(1);
    }
    let arg = args.as_slice();
    let mountpoint = arg
        .get(0)
        .and_then(|s| s.to_str())
        .expect("Missing mountpoint argument");
    let backing = arg
        .get(1)
        .and_then(|s| s.to_str())
        .expect("Missing backing directory argument");

    let res_target_backing = extract_hostname_and_path(backing);
    if res_target_backing.is_none() {
        eprintln!("Backing argument must be in the format hostname:directory_path");
        std::process::exit(1);
    }

    let (hostname, directory_path) = res_target_backing.unwrap();

    println!(
        "Mounting TULFS with server {} and directory path {}",
        hostname, directory_path
    );

    let backing_root = PathBuf::from(directory_path);

    let mut opts = vec![
        MountOption::FSName("TULFS".into()),
        MountOption::AutoUnmount,
        MountOption::DefaultPermissions,
    ];

    let tulfs = TULFS::new(hostname.to_string(), backing_root);

    if let Err(err) = fuser::mount2(tulfs, mountpoint, &opts) {
        eprintln!("Failed to mount filesystem: {}", err);
        std::process::exit(1);
    }
}
