pub mod io;
pub mod request;

use std::future::Future;

/// A trait for asynchronously providing items in a streaming fashion.
///
/// The Provider trait defines an interface for types that can asynchronously
/// produce a sequence of items. This is particularly useful for scenarios like:
/// - Reading from data streams
/// - Processing queue items
/// - Implementing iterator-like patterns for async operations
///
/// # Type Parameters
/// * `Item` - The type of items that this provider produces
pub trait Provider {
    /// The type of items that this provider produces
    type Item;

    /// Asynchronously fetches the next item from this provider.
    ///
    /// # Returns
    /// - `Some(Item)` if an item is available
    /// - `None` if there are no more items to provide
    ///
    /// # Notes
    /// - This method is expected to be non-blocking
    /// - The returned future must be `Send` to support usage across thread boundaries
    /// - Implementations should return `None` when they have no more items to provide
    fn fetch(&mut self) -> impl Future<Output = Option<Self::Item>> + Send;
}
