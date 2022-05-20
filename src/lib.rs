use crossbeam_channel::{bounded, unbounded};
use log::*;
use std::sync::Mutex;
use thiserror::Error;
use zip::ZipArchive;

use std::borrow::Cow;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::thread::{self, current};
use std::time::Duration;

mod local_fs;
mod zip_fs;

pub enum RecvMsg {
    ReadProgress(f32),
    ReadDone(Box<[u8]>),
    Error(VfsError),
    IsDirectory(usize),
}

#[derive(Error, Debug)]
pub enum InternalError {
    #[error("File Error)")]
    FileDirNotFound,
    #[error("File Error)")]
    FileError(#[from] std::io::Error),
    #[error("Send Error")]
    SendError(#[from] crossbeam_channel::SendError<RecvMsg>),
}

#[derive(Error, Debug)]
pub enum VfsError {
    /// Errors from std::io::Error
    #[error("File Error)")]
    FileError(#[from] std::io::Error),
}

/// File system implementations must implement this trait
pub trait VfsDriver: std::fmt::Debug {
    /// This indicates that the file system is remote (such as ftp, https) and has no local path
    fn is_remote(&self) -> bool;
    /// If the driver supports a certain url
    fn supports_url(&self, url: &str) -> bool;
    // Create a new instance given data. The VfsDriver will take ownership of the data
    fn create_instance(&self) -> Box<dyn VfsDriver>;
    // Get some data in and returns true if driver can be mounted from it
    fn can_load_from_data(&self, data: &[u8]) -> bool;
    // Create a new instance given data. The VfsDriver will take ownership of the data
    fn create_from_data(&self, data: Box<[u8]>) -> Option<VfsDriverType>;
    // Get some data in and returns true if driver can be mounted from it
    fn can_load_from_url(&self, url: &str) -> bool;
    /// Used when creating an instance of the driver with a path to load from
    fn create_from_url(&self, url: &str) -> Option<VfsDriverType>;
    /// Returns a handle which updates the progress and returns the loaded data. This will try to
    fn load_url(
        &mut self,
        path: &str,
        //msg: &crossbeam_channel::Sender<RecvMsg>,
    ) -> Result<RecvMsg, InternalError>;

    // get a file/directory listing for the driver
    fn get_directory_list(&self, path: &str) -> Vec<Node>;
}

pub struct Handle {
    pub recv: crossbeam_channel::Receiver<RecvMsg>,
}

pub enum VfsType {
    // Used for remote loading (such as ftp, http)
    Remote,
    //
    Streaming,
}

pub enum NodeType {
    Unknown,
    File,
    Directory,
    Archive,
    Other(usize),
}

impl Default for NodeType {
    fn default() -> Self {
        NodeType::Unknown
    }
}

// TODO: Move bunch of data out to arrays to reduce reallocs
#[derive(Default)]
pub struct Node {
    node_type: NodeType,
    name: String,
    driver_index: u32,
    parent: u32,
    nodes: Vec<u32>,
}

impl Node {
    fn new_directory_node(name: &str) -> Node {
        Node {
            node_type: NodeType::Directory,
            name: name.to_owned(),
            ..Default::default()
        }
    }

    fn new_file_node(name: &str) -> Node {
        Node {
            node_type: NodeType::File,
            name: name.to_owned(),
            ..Default::default()
        }
    }
}

type VfsDriverType = Box<dyn VfsDriver>;

#[derive(Default)]
struct VfsState {
    nodes: Vec<Node>,
    node_drivers: Vec<VfsDriverType>,
    drivers: Vec<VfsDriverType>,
}

impl VfsState {
    fn new() -> VfsState {
        let drivers: Vec<VfsDriverType> = vec![
            Box::new(zip_fs::ZipFs::new()),
            Box::new(local_fs::LocalFs::new()),
        ];

        VfsState {
            drivers,
            nodes: vec![Node::new_directory_node("root")],
            ..Default::default()
        }
    }
}

pub struct Vfs {
    //state: Arc<Mutex<VfsState>>,
    /// for sending messages to the main-thread
    main_send: crossbeam_channel::Sender<SendMsg>,
}

//pub type VfsInstance = Box<dyn VfsDriver>;

impl Vfs {
    /// Loads the first file given a url. If an known archive is encounterd the first file will be extracted
    /// (given if it has a file listing) and that will be returned until a file is encountered. If there are
    /// no files an error will/archive will be returned instead and the user code has to handle it
    pub fn load_url(&self, path: &str) -> Handle {
        let (thread_send, main_recv) = bounded::<RecvMsg>(1);

        self.main_send
            .send(SendMsg::LoadUrl(path.into(), 0, thread_send))
            .unwrap();

        Handle { recv: main_recv }
    }
}

pub enum SendMsg {
    // TODO: Proper error
    /// Send messages
    LoadUrl(String, u32, crossbeam_channel::Sender<RecvMsg>),
}

fn handle_error(e: InternalError, msg: &crossbeam_channel::Sender<RecvMsg>) {
    if let InternalError::FileError(e) = e {
        let file_error = format!("{:#?}", e);
        if let Err(send_err) = msg.send(RecvMsg::Error(e.into())) {
            error!(
                "evfs: Unable to send file error {:#?} to main thread due to {:#?}",
                file_error, send_err
            );
        }
    }
}

enum NodeInTree {
    NotFound,
    Incomplete(usize),
    Complete(usize),
}

fn is_path_in_tree(state: &VfsState, start_index: u32, path: &Path) -> NodeInTree {
    let comp = path.components();
    let mut index = start_index as usize;
    let nodes = &state.nodes;

    for component in comp {
        let node = &nodes[index];

        if let Component::Normal(p) = component {
            if let Some(found_index) = find_entry_in_node(node, nodes, &p.to_string_lossy()) {
                index = found_index;
            } else {
                // return the index where the search stopped within the tree
                return NodeInTree::Incomplete(index);
            }
        }
    }

    if start_index == 0 {
        NodeInTree::NotFound
    } else {
        NodeInTree::Complete(index)
    }
}

fn find_driver_for_path(drivers: &[VfsDriverType], path: &str) -> Option<VfsDriverType> {
    for d in drivers {
        if d.supports_url(path) {
            // Check if we can
            if d.can_load_from_url(path) {
                if let Some(new_driver) = d.create_from_url(path) {
                    return Some(new_driver);
                }
            }
        }
    }

    None
}

fn add_path_to_vfs(state: &mut VfsState, path: &str, driver: VfsDriverType) {}

/// Loading of urls works in the following way:
/// 1. First we start with the full urls for example: ftp://dir/foo.zip/test/bar.mod
/// 2. We try to load the url as is. In this case it will fail as only ftp://dir/foo.zip is present on the ftp server
/// 3. We now search backwards until we get get a file loaded (in this case ftp://dir/foo.zip)
/// 4. As we haven't fully resolved the full path yet, we will from this point search backwards again again with foo.zip
///    trying to resolve "foo.zip/test/bar.mod" which should succede in this case.
///    We repeat this process until everthing we are done.

fn load_new_url(state: &mut VfsState, path: &Path, str_path: &str) -> bool {
    let mut p = path.to_path_buf();
    let mut current_path = p.to_str().unwrap().to_owned();
    let mut driver = None;

    while !current_path.is_empty() {
        driver = find_driver_for_path(&state.drivers, &current_path);

        if driver.is_some() {
            break;
        }

        p.pop();
        current_path = p.to_str().unwrap().to_owned();
    }

    // Unable to load this path
    if current_path.is_empty() {
        return false;
    }

    true
}

/// We have a separate function for getting a root as this doesn't exist in Path. Returns
/// the starting of the relative paths
/*
fn get_root(path: &str) -> Option<usize> {
    // if we have internet style url just return that
    if let Some(url) = path.find("://") {
        return Some(url + 3);
    }

    let mut path = Path::new(path).components();

    match path.next() {
        // if we are here it means that we didn't encounter a Windows style prefix and assume it's unix style
        Some(Component::RootDir) => Some(1),
        Some(Component::Prefix(t)) => Some(t.as_os_str().len()),
        _ => None,
    }
}
*/

/// Loading of urls works in the following way:
/// 1. First we start with the full urls for example: ftp://dir/foo.zip/test/bar.mod
/// 2. We try to load the url as is. In this case it will fail as only ftp://dir/foo.zip is present on the ftp server
/// 3. We now search backwards until we get get a file loaded (in this case ftp://dir/foo.zip)
/// 4. As we haven't fully resolved the full path yet, we will from this point search backwards again again with foo.zip
///    trying to resolve "foo.zip/test/bar.mod" which should succede in this case.
///    We repeat this process until everthing we are done.

fn try_loading(
    state: &mut VfsState,
    node_index: usize,
    path_index: usize,
    components: &[Component],
) {
    let mut driver;
    // Construct the full path given the current node index
    let mut p: PathBuf = components.iter().collect();

    // if we aren't at node
    driver = &state.node_drivers[state.nodes[node_index].driver_index as usize];

    let mut current_path = p.to_str().unwrap().to_owned();

    /*
    while !current_path.is_empty() {
        if driver.load_url(&current_path) {}

        p.pop();
        current_path = p.to_str().unwrap().to_owned();
    }
    */
}

// TODO: uses a hashmap instead?
fn find_entry_in_node(node: &Node, nodes: &[Node], name: &str) -> Option<usize> {
    for n in &node.nodes {
        let index = *n as usize;
        let t = &nodes[index];
        if t.name == name {
            return Some(index);
        }
    }

    None
}

/// Loading of urls works in the following way:
/// 1. First we start with the full urls for example: ftp://dir/foo.zip/test/bar.mod
/// 2. We try to load the url as is. In this case it will fail as only ftp://dir/foo.zip is present on the ftp server
/// 3. We now search backwards until we get get a file loaded (in this case ftp://dir/foo.zip)
/// 4. As we haven't fully resolved the full path yet, we will from this point search backwards again again with foo.zip
///    trying to resolve "foo.zip/test/bar.mod" which should succede in this case.
///    We repeat this process until everthing we are done.
pub(crate) fn load_url_internal(state: &mut VfsState, path: &str) -> bool {
    let had_prefix = false;
    let mut index = 0;

    let comp: Vec<Component> = Path::new(path).components().collect();

    for (i, c) in comp.iter().enumerate() {
        match c {
            Component::RootDir => {
                if !had_prefix {
                    let node = &state.nodes[index];
                    // Check if the path exists in the tree, if so we go on searching
                    if let Some(entry) = find_entry_in_node(node, &state.nodes, "/") {
                        index = entry;
                    } else {
                        // this path was not found in the vfs, so we need to try loading it
                        //try_loading(state, index, &comp[i..]);
                    }
                }
            }

            Component::Prefix(t) => {
                let prefix_name = t.as_os_str().to_string_lossy();
                let node = &state.nodes[index];
                // Check if the path exists in the tree, if so we go on searching
                if let Some(entry) = find_entry_in_node(node, &state.nodes, &prefix_name) {
                    index = entry;
                } else {
                    // this path was not found in the vfs, so we need to try loading it
                }
            }

            Component::Normal(t) => {
                let node = &state.nodes[index];
                // Check if the path exists in the tree, if so we go on searching
                if let Some(entry) = find_entry_in_node(node, &state.nodes, &t.to_string_lossy()) {
                    index = entry;
                } else {
                    // this path was not found in the vfs, so we need to try loading it
                }
            }

            e => unimplemented!("{:?}", e),
        }
    }

    true
}

fn get_component_name<'a>(component: &'a Component, had_prefix: &mut bool) -> Cow<'a, str> {
    match component {
        Component::RootDir => {
            if !*had_prefix {
                "/".into()
            } else {
                "".into()
            }
        }

        Component::Prefix(t) => {
            *had_prefix = true;
            t.as_os_str().to_string_lossy()
        }
        Component::Normal(t) => t.to_string_lossy(),
        e => unimplemented!("{:?}", e),
    }
}

// Add a new node to the vfs at a specific index and get the new node index back
fn add_new_node(state: &mut VfsState, index: usize, new_node: Node) -> usize {
    //let mut node = &state.nodes[index];
    let new_index = state.nodes.len();
    state.nodes.push(new_node);
    state.nodes[index].nodes.push(new_index as u32);
    new_index
}

/// Walk backwards until we can load a path.
fn try_loading_test(
    state: &mut VfsState,
    start_index: usize,
    components: &[Component],
) -> (usize, usize) {
    let mut p: PathBuf = components.iter().collect();

    let mut current_path = p.to_str().unwrap().to_owned();
    let mut driver = None;
    let mut level = 0;
    let mut node_index = start_index;
    let mut has_prefix = false;

    let path_index = components.len();

    println!("current path {}", current_path);

    while !current_path.is_empty() {
        driver = find_driver_for_path(&state.drivers, &current_path);

        println!("driver for path {} {}", current_path, driver.is_some());

        if driver.is_some() {
            break;
        }

        p.pop();
        current_path = p.to_str().unwrap().to_owned();
        level += 1;
    }

    if let Some(driver) = driver {
        let driver_index = state.node_drivers.len();
        state.node_drivers.push(driver);

        let components = p.components();

        for c in components {
            let new_node = Node {
                node_type: NodeType::Unknown,
                parent: node_index as _,
                driver_index: driver_index as u32,
                name: get_component_name(&c, &mut has_prefix).to_string(),
                ..Default::default()
            };

            node_index = add_new_node(state, node_index, new_node);
        }
    }

    // return the next level to process the path at
    (path_index - level, node_index)
}

#[derive(PartialEq)]
enum LoadState {
    FindNode,
    FindDriverUrl,
    FindDriverData,
    LoadFromDriver,
}

pub(crate) fn load(vfs: &mut VfsState, path: &str) -> bool {
    let mut had_prefix = false;
    let mut node_index = 0;

    // temp
    //let (main_send, thread_recv) = unbounded::<SendMsg>();

    let components: Vec<Component> = Path::new(path).components().collect();
    let comp_len = components.len();

    let mut i = 0;
    let mut driver_index = None;
    let mut state = LoadState::FindNode;
    let mut data: Option<Box<[u8]>> = None;

    loop {
        match state {
            // Search for node in the vfs
            LoadState::FindNode => {
                // Search for node in the vfs
                for (t, c) in components[i..].iter().enumerate() {
                    let node = &vfs.nodes[node_index];
                    let component_name = get_component_name(&c, &mut had_prefix);
                    if let Some(entry) = find_entry_in_node(node, &vfs.nodes, &component_name) {
                        //println!("found entry! {}: {}", component_name, entry);
                        node_index = entry;
                        i += 1;
                        // If we don't have any driver yet and we didn't find a path we must search for a driver
                    } else if driver_index.is_none() {
                        state = LoadState::FindDriverUrl;
                    } else {
                        state = LoadState::LoadFromDriver;
                    }
                }
            }

            // Walk the url backwards and try to find a driver
            LoadState::FindDriverUrl => {
                let mut p: PathBuf = components[i..].iter().collect();

                let mut current_path = p.to_str().unwrap().to_owned();
                let mut level = 0;
                let mut driver = None;

                println!("current path {}", current_path);

                while !current_path.is_empty() {
                    driver = find_driver_for_path(&vfs.drivers, &current_path);

                    println!("driver for path {} {}", current_path, driver.is_some());

                    if driver.is_some() {
                        break;
                    }

                    p.pop();
                    current_path = p.to_str().unwrap().to_owned();
                    level += 1;
                }

                if let Some(d) = driver {
                    // If we found a driver we mount it inside the vfs
                    let mut current_index = node_index;
                    let mut prefix = false;
                    let d_index = vfs.node_drivers.len();

                    driver_index = Some(d_index);
                    vfs.node_drivers.push(d);

                    let comp = p.components();

                    for c in comp {
                        let new_node = Node {
                            node_type: NodeType::Unknown,
                            parent: current_index as _,
                            driver_index: d_index as u32,
                            name: get_component_name(&c, &mut prefix).to_string(),
                            ..Default::default()
                        };

                        current_index = add_new_node(vfs, current_index, new_node);
                    }

                    // Start from the index where the new driver has been mounted
                    node_index = current_index;
                    i += comp_len - level;

                    state = LoadState::LoadFromDriver;
                } else {
                    // Unable to find a driver to load
                    return false;
                }
            }

            LoadState::FindDriverData => {
                let mut p: PathBuf = components[i..].iter().collect();
                let mut current_path = p.to_str().unwrap().to_owned();
                println!("Searching for {} in data", current_path);
                let node_data = data.as_ref().unwrap();

                for d in &vfs.drivers {
                    if !d.can_load_from_data(&node_data) {
                        continue;
                    }

                    // TODO: Fix this clone
                    if let Some(new_driver) = d.create_from_data(node_data.clone()) {
                        let d_index = vfs.node_drivers.len();
                        driver_index = Some(d_index);
                        vfs.node_drivers.push(new_driver);

                        // Update the current node with new driver
                        vfs.nodes[node_index].driver_index = d_index as _;

                        state = LoadState::LoadFromDriver;
                    }

                    break;
                }
            }

            LoadState::LoadFromDriver => {
                let mut p: PathBuf = components[i..].iter().collect();
                let mut current_path = p.to_str().unwrap().to_owned();
                let d_index = driver_index.unwrap();
                let d = &mut vfs.node_drivers[d_index];

                println!(">> Lodaing with driver {}", d_index);

                // walkbackwards from the current path and try to load the data

                let mut p: PathBuf = components[i..].iter().collect();

                let mut current_path = p.to_str().unwrap().to_owned();
                let mut level = 0;

                println!("current path {}", current_path);

                while !current_path.is_empty() {
                    match d.load_url(&current_path) {
                        Err(e) => {
                            println!("{:?}", e);
                            //return false;
                        }

                        Ok(msg) => {
                            match msg {
                                RecvMsg::IsDirectory(_) => {
                                    println!("path is dir {}", current_path);
                                    // TODO: Insert into tree
                                    return true;
                                }

                                RecvMsg::ReadDone(in_data) => {
                                    println!("Read data to memory: {}", current_path);
                                    // if level is 0 then we are done, otherwise we have to continue
                                    // TODO: If user has "scan" on data we need to continue here as well
                                    if level == 0 {
                                        return true;
                                    } else {
                                        let comp = p.components();
                                        let mut current_index = node_index;
                                        let mut prefix = false;
                                        let mut count = 0;

                                        for c in comp {
                                            let new_node = Node {
                                                node_type: NodeType::Unknown,
                                                parent: current_index as _,
                                                driver_index: d_index as u32,
                                                name: get_component_name(&c, &mut prefix)
                                                    .to_string(),
                                                ..Default::default()
                                            };

                                            current_index =
                                                add_new_node(vfs, current_index, new_node);
                                            count += 1;
                                        }

                                        node_index = current_index;
                                        i += count;

                                        // Add new nodes to the vfs
                                        data = Some(in_data);
                                        state = LoadState::FindDriverData;
                                        break;
                                    }
                                }

                                _ => (),
                            }
                        }
                    }

                    p.pop();
                    current_path = p.to_str().unwrap().to_owned();
                    level += 1;
                }

                if current_path.is_empty() && state == LoadState::LoadFromDriver {
                    println!("Unable to find url that was searched for");
                    return false;
                }

                //return true;
            }
        }

        //let component = components[i];
        //let component_name = get_component_name(&component, &mut had_prefix);
        //let node = &state.nodes[index];
    }

    /*
    loop {
        let component = components[i];
        let component_name = get_component_name(&component, &mut had_prefix);
        let node = &state.nodes[index];

        if let Some(entry) = find_entry_in_node(node, &state.nodes, &component_name) {
            println!("found entry! {}: {}", component_name, entry);
            index = entry;
            i += 1;
        } else {
            let mut p: PathBuf = components[i..].iter().collect();

            let mut current_path = p.to_str().unwrap().to_owned();
            let mut driver = None;
            let mut level = 0;
            let mut node_index = start_index;

            let path_index = components.len();

            println!("current path {}", current_path);

            while !current_path.is_empty() {
                driver = find_driver_for_path(&state.drivers, &current_path);

                println!("driver for path {} {}", current_path, driver.is_some());

                if driver.is_some() {
                    break;
                }

                p.pop();
                current_path = p.to_str().unwrap().to_owned();
                level += 1;
            }

            if let Some(driver) = driver {
                let driver_index = state.node_drivers.len();
                state.node_drivers.push(driver);

                let components = p.components();

                for c in components {
                    let new_node = Node {
                        node_type: NodeType::Unknown,
                        parent: node_index as _,
                        driver_index: driver_index as u32,
                        name: get_component_name(&c, &mut has_prefix).to_string(),
                        ..Default::default()
                    };

                    node_index = add_new_node(state, node_index, new_node);
                }
            }

            // return the next level to process the path at
            (path_index - level, node_index)

            /*
            let t = try_loading_test(state, index, &components[i..]);
            i += t.0;
            index = t.1;

            println!("{} {} len {}", i, index, components.len());
            */
        }

        if i == components.len() {
            break;
        }
    }
    */

    true
}

/*
fn handle_msg(state: &mut VfsState, _name: &str, msg: &SendMsg) {
    match msg {
        SendMsg::LoadUrl(path, node_index, msg) => {
            let p = Path::new(path);

            match is_path_in_tree(state, *node_index, p) {
                NodeInTree::NotFound => load_new_url(state, p, path),
                NodeInTree::Complete(_index) => unimplemented!(),
                NodeInTree::Incomplete(_index) => unimplemented!(),
            }

            /*

            // TODO: Support parsing the url in sub-chunks i.e mydir/foo.zip/dir/foo.rar/file
            for d in &drivers {
                // Found a loader that works
                let loader = d.sreate_instance();

                match loader.load_url(path, msg) {
                    Err(e) => {
                        handle_error(e, msg);
                        return;
                    }

                    msg.send(rec_msg).unwrap(),
                }
            }
            */
        }
    }
}
*/

impl Vfs {
    //pub fn new(vfs_drivers: Option<&[Box<dyn VfsDriver>]>) -> Vfs {
    pub fn new() -> Vfs {
        let (main_send, thread_recv) = unbounded::<SendMsg>();

        //let state = Arc::new(Mutex::new(vfs_state));
        let thread_recv_0 = thread_recv.clone();

        // Setup worker thread
        thread::Builder::new()
            .name("vfs_worker".to_string())
            .spawn(move || {
                let mut state = VfsState::new();

                /*
                while let Ok(msg) = thread_recv_0.recv() {
                    handle_msg(&mut state, "vfs_worker", &msg);
                }
                 */
            })
            .unwrap();

        Vfs { main_send }
    }
}

impl Default for Vfs {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn print_tree(state: &VfsState, index: u32, parent: u32, indent: usize) {
        let node = &state.nodes[index as usize];

        println!(
            "{:indent$} {} driver {}",
            "",
            node.name,
            node.driver_index,
            indent = indent
        );

        for n in &node.nodes {
            print_tree(state, *n, node.parent, indent + 1);
        }
    }

    /*
    #[test]
    fn vfs_state_load_local() {
        let mut state = VfsState::new();
        let path = std::fs::canonicalize("Cargo.toml").unwrap();
        load(&mut state, &path.to_string_lossy());

        let path = std::fs::canonicalize("Cargo.lock").unwrap();
        load(&mut state, &path.to_string_lossy());

        print_tree(&state, 0, 0, 0);
    }
    */

    #[test]
    fn vfs_state_load_zip() {
        let mut state = VfsState::new();
        let mut path = std::fs::canonicalize("data/a.zip").unwrap();
        path = path.join("beat.zip/foo/6beat.mod");
        println!("{:?}", path);

        load(&mut state, &path.to_string_lossy());

        print_tree(&state, 0, 0, 0);
    }

    /*
    #[test]
    fn check_directory() {
        let vfs = Vfs::new();

        let handle = vfs.load_url("Cargo.toml");

        let mut found_dir = false;

        for _ in 0..10 {
            if let Ok(data) = handle.recv.try_recv() {
                match data {
                    RecvMsg::IsDirectory(_) => found_dir = true,
                    RecvMsg::Error(e) => panic!("{:?}", e),
                    _ => (),
                }
            }

            thread::sleep(Duration::from_millis(10));
        }

        assert!(found_dir);
    }
    */

    /*
    #[test]
    fn test_get_directory_ftp() {
        let vfs = Vfs::new(None);
        let mut got_archive = false;
        let handle = vfs.load_url("ftp://ftp.modland.com/pub/modules");

        for _ in 0..10 {
            match handle.recv.try_recv() {
                Ok(data) => match data {
                    RecvMsg::Archive(_) => got_archive = true,
                },
                _ => (),
            }

            thread::sleep(time::Duration::from_millis(200));
        }

        assert_eq!(got_archive, true);
    }
    */
}
