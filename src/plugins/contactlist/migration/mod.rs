use sea_orm_migration::prelude::*;

mod m20220102_000000_contact_list;
mod m20220102_000001_contact;
mod m20220102_000002_contact_email;
mod m20220102_000003_contact_phone;
mod m20220102_000004_contact_address;

pub struct ContactListMigrator;

impl ContactListMigrator {
    /// Get all contact list plugin migrations
    pub fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        vec![
            Box::new(m20220102_000000_contact_list::Migration),
            Box::new(m20220102_000001_contact::Migration),
            Box::new(m20220102_000002_contact_email::Migration),
            Box::new(m20220102_000003_contact_phone::Migration),
            Box::new(m20220102_000004_contact_address::Migration),
        ]
    }
}
