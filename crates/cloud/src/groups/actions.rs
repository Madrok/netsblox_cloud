use std::collections::HashMap;

use futures::TryStreamExt;
use mongodb::{bson::doc, options::ReturnDocument, Collection};
use netsblox_cloud_common::{api, Group, User};

use crate::auth;
use crate::errors::{InternalError, UserError};

pub(crate) struct GroupActions {
    groups: Collection<Group>,
    users: Collection<User>,
}

impl GroupActions {
    pub(crate) async fn create_group(
        &self,
        eu: &auth::EditUser,
        name: &str,
    ) -> Result<api::Group, UserError> {
        let group = Group::new(eu.username.to_owned(), name.to_owned());
        let query = doc! {"name": &group.name, "owner": &group.owner};
        let update = doc! {"$setOnInsert": &group};
        let options = mongodb::options::UpdateOptions::builder()
            .upsert(true)
            .build();

        let result = self
            .groups
            .update_one(query, update, options)
            .await
            .map_err(InternalError::DatabaseConnectionError)?;

        if result.matched_count == 1 {
            Err(UserError::GroupExistsError)
        } else {
            let group: api::Group = group.into();
            Ok(group)
        }
    }

    pub(crate) async fn list_groups(
        &self,
        vg: &auth::users::ViewUser,
    ) -> Result<Vec<api::Group>, UserError> {
        let query = doc! {"owner": &vg.username};
        let cursor = self
            .groups
            .find(query, None)
            .await
            .map_err(InternalError::DatabaseConnectionError)?;
        let groups: Vec<api::Group> = cursor
            .try_collect::<Vec<_>>()
            .await
            .map_err(InternalError::DatabaseConnectionError)?
            .into_iter()
            .map(|g| g.into())
            .collect();

        Ok(groups)
    }

    pub(crate) async fn view_group(
        &self,
        vg: &auth::groups::ViewGroup,
    ) -> Result<api::Group, UserError> {
        let query = doc! {"id": &vg.id};
        let group = self
            .groups
            .find_one(query, None)
            .await
            .map_err(InternalError::DatabaseConnectionError)?
            .ok_or(UserError::GroupNotFoundError)?;

        Ok(group.into())
    }

    pub(crate) async fn rename_group(
        &self,
        eg: &auth::groups::EditGroup,
        name: &str,
    ) -> Result<api::Group, UserError> {
        let query = doc! {"id": &eg.id};
        let update = doc! {"$set": {"name": &name}};
        let options = mongodb::options::FindOneAndUpdateOptions::builder()
            .return_document(ReturnDocument::After)
            .build();

        let group = self
            .groups
            .find_one_and_update(query, update, options)
            .await
            .map_err(InternalError::DatabaseConnectionError)?
            .ok_or(UserError::GroupNotFoundError)?;

        Ok(group.into())
    }

    pub(crate) async fn set_group_hosts(
        &self,
        eg: &auth::groups::EditGroup,
        hosts: &[api::ServiceHost],
    ) -> Result<api::Group, UserError> {
        let query = doc! {"id": &eg.id};
        let update = doc! {"$set": {"servicesHosts": hosts}};
        let options = mongodb::options::FindOneAndUpdateOptions::builder()
            .return_document(ReturnDocument::After)
            .build();
        let group = self
            .groups
            .find_one_and_update(query, update, options)
            .await
            .map_err(InternalError::DatabaseConnectionError)?
            .ok_or(UserError::GroupNotFoundError)?;

        Ok(group.into())
    }

    pub(crate) async fn get_service_settings(
        &self,
        vg: &auth::groups::ViewGroup,
    ) -> Result<HashMap<String, String>, UserError> {
        let query = doc! {"id": &vg.id};
        let group = self
            .groups
            .find_one(query, None)
            .await
            .map_err(InternalError::DatabaseConnectionError)?
            .ok_or(UserError::UserNotFoundError)?;

        Ok(group.service_settings)
    }

    pub(crate) async fn set_service_settings(
        &self,
        vg: &auth::groups::EditGroup,
        host: &str,
        settings: &str,
    ) -> Result<api::Group, UserError> {
        let query = doc! {"id": &vg.id};
        let update = doc! {"$set": {format!("serviceSettings.{}", &host): settings}};
        let options = mongodb::options::FindOneAndUpdateOptions::builder()
            .return_document(ReturnDocument::After)
            .build();

        let group = self
            .groups
            .find_one_and_update(query, update, options)
            .await
            .map_err(InternalError::DatabaseConnectionError)?
            .ok_or(UserError::GroupNotFoundError)?;

        Ok(group.into())
    }

    pub(crate) async fn delete_service_settings(
        &self,
        vg: &auth::groups::EditGroup,
        host: &str,
    ) -> Result<api::Group, UserError> {
        let query = doc! {"id": &vg.id};
        let update = doc! {"$unset": {format!("serviceSettings.{}", &host): true}};
        let options = mongodb::options::FindOneAndUpdateOptions::builder()
            .return_document(ReturnDocument::After)
            .build();

        let group = self
            .groups
            .find_one_and_update(query, update, options)
            .await
            .map_err(InternalError::DatabaseConnectionError)?
            .ok_or(UserError::GroupNotFoundError)?;

        Ok(group.into())
    }

    pub(crate) async fn delete_group(
        &self,
        vg: &auth::groups::DeleteGroup,
    ) -> Result<api::Group, UserError> {
        let query = doc! {"id": &vg.id};
        let group: api::Group = self
            .groups
            .find_one_and_delete(query, None)
            .await
            .map_err(InternalError::DatabaseConnectionError)?
            .ok_or(UserError::GroupNotFoundError)?
            .into();

        Ok(group.into())
    }

    // TODO: move this to the user actions??
    pub(crate) async fn list_members(
        &self,
        vg: &auth::groups::ViewGroup,
    ) -> Result<Vec<api::User>, UserError> {
        let query = doc! {"groupId": &vg.id};
        // TODO:
        let cursor = self
            .users
            .find(query, None)
            .await
            .map_err(InternalError::DatabaseConnectionError)?;
        let members: Vec<api::User> = cursor
            .try_collect::<Vec<_>>()
            .await
            .map_err(InternalError::DatabaseConnectionError)?
            .into_iter()
            .map(|u| u.into())
            .collect();

        Ok(members)
    }
}
