/// ContactList Plugin - CardDAV-compatible contact management
///
/// This plugin provides a vCard 4.0 compliant contact list implementation
/// that can be used with CardDAV clients.
///
/// Enable with: `--features plugin-contactlist`

pub mod entity;
pub mod migration;

pub use migration::ContactListMigrator;
