


/// File system implementations must implement this trait
pub trait VfsDriver: Sync + Send + Clone {
    /// This indicates that the file system is remote (such as ftp, https) and has no local path
    fn is_remote(&self) -> bool;
    /// If the compressor supports a certain extension
    fn supports_file_ext(&self, file_ext: &str) -> bool;
    // Instance used to clone from
    fn new_instance() -> Box<dyn VfsDriver>;
    /// Used when creating an instance of the driver with a path to load from
    fn new_from_path(&self, path: &str) -> Result<Box<dyn VfsDriver>, VfsError>;
    fn new_from_path(&self, path: &str) -> Result<Box<dyn VfsDriver>, VfsError>;
    /// Returns a handle which updates the progress and returns the loaded data. This will try to
    fn load_url(
        &self,
        path: &str,
        msg: &crossbeam_channel::Sender<RecvMsg>,
    ) -> Result<Box<[u8]>, InternalError>;

    fn get_file_list(node: usize, vfs: &mut Vfs) -> Vec<Nodes>,
}


pub enum NodeType {
    Unknown,
    File,
    Directory,
    Other(usize),
}

// TODO: Move bunch of data out to arrays to reduce reallocs
struct Node {
    node_type: NodeType,
    name_index: String,
    // Driver index
    driver_index: usize,
    // Driver that may have to be resolved to load the data in this node
    dependent_driver: usize,
    parent: usize,
    nodes: Vec<usize>,
}

pub struct Vfs {
    nodes: Vec<Node>,
    /// Used
    node_drivers: Vec<Arc<dyn VfsDriver>>,
    drivers: Vec<Box<dyn VfsDriver>>,
}

impl Vfs {
    pub fn new() -> Vfs {
        let mut drivers = Vec::new();

        drivers.push(Arc::new(LocalFs::new_instance())));

        Vfs {
            nodes: Vec::new(),
            node_drivers: Vec::new(),
            drivers,
        }
    }
}  