use sea_orm_migration::prelude::*;

mod m20220101_000001_user;
mod m20220101_000002_group;
mod m20220101_000003_group_user;
mod m20220101_000004_tag;
mod m20220101_000005_storage;
mod m20220101_000006_entry;
mod m20220101_000007_shared;
mod m20220101_000008_meta;
mod m20220101_000010_token;
mod m20220101_000011_photo;
mod m20220101_000012_meta_custom_required;
mod m20220101_000013_entry_skip_plugins;
mod m20220101_000014_entry_drop_kind;
mod m20220101_000015_storage_ignore_patterns;
mod m20220101_000016_contact;
mod m20220101_000017_face_reference;
mod m20220101_000018_face_reference_model_meta;
mod m20220101_000019_shared_owner;

pub struct Migrator;

#[async_trait::async_trait]
impl MigratorTrait for Migrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        vec![
            Box::new(m20220101_000001_user::Migration),
            Box::new(m20220101_000002_group::Migration),
            Box::new(m20220101_000003_group_user::Migration),
            Box::new(m20220101_000004_tag::Migration),
            Box::new(m20220101_000005_storage::Migration),
            Box::new(m20220101_000006_entry::Migration),
            Box::new(m20220101_000007_shared::Migration),
            Box::new(m20220101_000008_meta::Migration),
            Box::new(m20220101_000010_token::Migration),
            Box::new(m20220101_000011_photo::Migration),
            Box::new(m20220101_000012_meta_custom_required::Migration),
            Box::new(m20220101_000013_entry_skip_plugins::Migration),
            Box::new(m20220101_000014_entry_drop_kind::Migration),
            Box::new(m20220101_000015_storage_ignore_patterns::Migration),
            Box::new(m20220101_000016_contact::Migration),
            Box::new(m20220101_000017_face_reference::Migration),
            Box::new(m20220101_000018_face_reference_model_meta::Migration),
            Box::new(m20220101_000019_shared_owner::Migration),
        ]
    }
}
