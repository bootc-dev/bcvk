use std::ffi::OsString;
use std::os::unix::io::AsFd;
use std::path::{Path, PathBuf};

use cap_std::fs::Dir;

#[derive(Debug)]
pub struct FileEntry {
    pub host_path: PathBuf,
    pub size: u64,
    pub mode: u32,
    pub uid: u32,
    pub gid: u32,
    pub mtime: u64,
    pub nlink: u32,
    pub inode_id: u64,
}

#[derive(Debug)]
pub struct DirInfo {
    pub name: OsString,
    pub mode: u32,
    pub uid: u32,
    pub gid: u32,
    pub mtime: u64,
    pub inode_id: u64,
    pub parent_inode_id: u64,
    pub children: Vec<ChildRef>,
}

#[derive(Debug)]
pub struct SymlinkEntry {
    pub name: Vec<u8>,
    pub target: Vec<u8>,
    pub mode: u32,
    pub uid: u32,
    pub gid: u32,
    pub mtime: u64,
    pub inode_id: u64,
}

/// Child entry in a directory: either a file index, dir index, or symlink index
#[derive(Debug, Clone, Copy)]
pub enum ChildRef {
    File(usize),
    Dir(usize),
    Symlink(usize),
}

#[derive(Debug)]
pub struct WalkResult {
    pub dirs: Vec<DirInfo>,
    pub files: Vec<FileEntry>,
    pub symlinks: Vec<SymlinkEntry>,
}

pub fn walk_directory(root_dir: &Dir, root_path: &Path) -> std::io::Result<WalkResult> {
    let mut result = WalkResult {
        dirs: Vec::new(),
        files: Vec::new(),
        symlinks: Vec::new(),
    };
    let mut next_inode: u64 = 0;

    walk_recursive(root_dir, root_path, &mut result, &mut next_inode, 0)?;
    Ok(result)
}

fn statat(dir: &Dir, name: &Path) -> std::io::Result<rustix::fs::Stat> {
    rustix::fs::statat(dir.as_fd(), name, rustix::fs::AtFlags::SYMLINK_NOFOLLOW)
        .map_err(std::io::Error::from)
}

fn walk_recursive(
    cap_dir: &Dir,
    current_path: &Path,
    result: &mut WalkResult,
    next_inode: &mut u64,
    parent_inode_id: u64,
) -> std::io::Result<usize> {
    let dir_stat = rustix::fs::fstat(cap_dir.as_fd()).map_err(std::io::Error::from)?;
    let dir_inode = *next_inode;
    *next_inode += 1;

    let di = result.dirs.len();
    result.dirs.push(DirInfo {
        name: current_path.file_name().unwrap_or_default().to_os_string(),
        mode: dir_stat.st_mode,
        uid: dir_stat.st_uid,
        gid: dir_stat.st_gid,
        mtime: dir_stat.st_mtime as u64,
        inode_id: dir_inode,
        parent_inode_id,
        children: Vec::new(),
    });

    let mut entries: Vec<_> = cap_dir.entries()?.filter_map(|e| e.ok()).collect();
    entries.sort_by_key(|e| e.file_name());

    for entry in entries {
        let name = entry.file_name();
        let name_path: &Path = name.as_ref();
        let stat = statat(cap_dir, name_path)?;
        let mode = stat.st_mode;
        let ft = rustix::fs::FileType::from_raw_mode(mode);

        if ft == rustix::fs::FileType::Directory {
            let child_dir = cap_dir.open_dir(&name)?;
            let child_path = current_path.join(&name);
            let child_di = walk_recursive(&child_dir, &child_path, result, next_inode, dir_inode)?;
            result.dirs[di].children.push(ChildRef::Dir(child_di));
        } else if ft == rustix::fs::FileType::Symlink {
            let target = rustix::fs::readlinkat(cap_dir.as_fd(), name_path, [])?;
            let target_bytes = target.into_bytes();
            let name_bytes = name.as_encoded_bytes().to_vec();
            let si = result.symlinks.len();
            let inode = *next_inode;
            *next_inode += 1;
            result.symlinks.push(SymlinkEntry {
                name: name_bytes,
                target: target_bytes,
                mode,
                uid: stat.st_uid,
                gid: stat.st_gid,
                mtime: stat.st_mtime as u64,
                inode_id: inode,
            });
            result.dirs[di].children.push(ChildRef::Symlink(si));
        } else if ft == rustix::fs::FileType::RegularFile {
            let fi = result.files.len();
            let inode = *next_inode;
            *next_inode += 1;
            result.files.push(FileEntry {
                host_path: current_path.join(&name),
                size: stat.st_size as u64,
                mode,
                uid: stat.st_uid,
                gid: stat.st_gid,
                mtime: stat.st_mtime as u64,
                nlink: stat.st_nlink as u32,
                inode_id: inode,
            });
            result.dirs[di].children.push(ChildRef::File(fi));
        }
    }

    Ok(di)
}
