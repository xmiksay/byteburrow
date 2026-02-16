use sea_orm_migration::prelude::*;

mod m20220101_000001_user;
mod m20220101_000002_group;
mod m20220101_000003_group_user;
mod m20220101_000004_tag;
mod m20220101_000005_storage;
mod m20220101_000006_entry;
mod m20220101_000007_shared;
mod m20220101_000010_token;
mod m20220101_000011_photo;

// Plugin migrations (feature-gated)
#[cfg(feature = "plugin-contactlist")]
use crate::plugins::contactlist::ContactListMigrator;

pub struct Migrator;

#[async_trait::async_trait]
impl MigratorTrait for Migrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        #[allow(unused_mut)]
        let mut migrations: Vec<Box<dyn MigrationTrait>> = vec![
            // Core migrations
            Box::new(m20220101_000001_user::Migration),
            Box::new(m20220101_000002_group::Migration),
            Box::new(m20220101_000003_group_user::Migration),
            Box::new(m20220101_000004_tag::Migration),
            Box::new(m20220101_000005_storage::Migration),
            Box::new(m20220101_000006_entry::Migration),
            Box::new(m20220101_000007_shared::Migration),
            Box::new(m20220101_000010_token::Migration),
            Box::new(m20220101_000011_photo::Migration),
        ];

        // Plugin migrations (conditionally included based on features)
        #[cfg(feature = "plugin-contactlist")]
        {
            migrations.extend(ContactListMigrator::migrations());
        }

        migrations
    }
}
