use std::ffi::OsString;
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

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

pub fn walk_directory(root: &Path) -> std::io::Result<WalkResult> {
    let mut result = WalkResult {
        dirs: Vec::new(),
        files: Vec::new(),
        symlinks: Vec::new(),
    };
    let mut next_inode: u64 = 0;

    walk_recursive(root, root, &mut result, &mut next_inode, 0)?;
    Ok(result)
}

fn walk_recursive(
    root: &Path,
    dir: &Path,
    result: &mut WalkResult,
    next_inode: &mut u64,
    parent_inode_id: u64,
) -> std::io::Result<usize> {
    let meta = fs::symlink_metadata(dir)?;
    let dir_inode = *next_inode;
    *next_inode += 1;

    let di = result.dirs.len();
    result.dirs.push(DirInfo {
        name: dir.file_name().unwrap_or_default().to_os_string(),
        mode: meta.mode(),
        uid: meta.uid(),
        gid: meta.gid(),
        mtime: meta.mtime() as u64,
        inode_id: dir_inode,
        parent_inode_id,
        children: Vec::new(),
    });

    let mut entries: Vec<_> = fs::read_dir(dir)?.filter_map(|e| e.ok()).collect();
    entries.sort_by_key(|e| e.file_name());

    for entry in entries {
        let path = entry.path();
        let meta = fs::symlink_metadata(&path)?;
        let ft = meta.file_type();

        if ft.is_dir() {
            let child_di = walk_recursive(root, &path, result, next_inode, dir_inode)?;
            result.dirs[di].children.push(ChildRef::Dir(child_di));
        } else if ft.is_symlink() {
            let target = fs::read_link(&path)?;
            let target_bytes = target.as_os_str().as_encoded_bytes().to_vec();
            let name = entry.file_name().as_encoded_bytes().to_vec();
            let si = result.symlinks.len();
            let inode = *next_inode;
            *next_inode += 1;
            result.symlinks.push(SymlinkEntry {
                name,
                target: target_bytes,
                mode: meta.mode(),
                uid: meta.uid(),
                gid: meta.gid(),
                mtime: meta.mtime() as u64,
                inode_id: inode,
            });
            result.dirs[di].children.push(ChildRef::Symlink(si));
        } else if ft.is_file() {
            let fi = result.files.len();
            let inode = *next_inode;
            *next_inode += 1;
            result.files.push(FileEntry {
                host_path: path,
                size: meta.len(),
                mode: meta.mode(),
                uid: meta.uid(),
                gid: meta.gid(),
                mtime: meta.mtime() as u64,
                nlink: meta.nlink() as u32,
                inode_id: inode,
            });
            result.dirs[di].children.push(ChildRef::File(fi));
        }
    }

    Ok(di)
}
