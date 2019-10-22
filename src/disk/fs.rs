/// Trait for performing operations on some file system.
/// Provides the necessary abstractions for handling files
pub trait FileSystem {
    /// Some file object.
    type File;
}
