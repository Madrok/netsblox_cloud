use actix_session::Session;
use actix_web::{get, post};
use actix_web::{web, HttpResponse};
use futures::TryStreamExt;
use mongodb::bson::doc;

use crate::app_data::AppData;
use crate::common::{api, api::InvitationState, api::ProjectId, CollaborationInvite};
use crate::errors::{InternalError, UserError};
use crate::network;
use crate::users::ensure_can_edit_user;
use mongodb::options::{FindOneAndUpdateOptions, ReturnDocument};

#[get("/user/{recipient}/")]
async fn list_invites(
    app: web::Data<AppData>,
    path: web::Path<(String,)>,
    session: Session,
) -> Result<HttpResponse, UserError> {
    let (recipient,) = path.into_inner();
    ensure_can_edit_user(&app, &session, &recipient).await?;

    let query = doc! {"recipient": recipient};
    let cursor = app.collab_invites.find(query, None).await.unwrap();
    let invites: Vec<api::CollaborationInvite> = cursor
        .try_collect::<Vec<_>>()
        .await
        .unwrap()
        .into_iter()
        .map(|invite| invite.into())
        .collect();

    Ok(HttpResponse::Ok().json(invites))
}

#[post("/{project_id}/invite/{recipient}")]
async fn send_invite(
    app: web::Data<AppData>,
    session: Session,
    path: web::Path<(ProjectId, String)>,
) -> Result<HttpResponse, UserError> {
    let (project_id, recipient) = path.into_inner();

    let query = doc! {"id": &project_id};
    let metadata = app
        .project_metadata
        .find_one(query, None)
        .await
        .map_err(InternalError::DatabaseConnectionError)?
        .ok_or(UserError::ProjectNotFoundError)?;

    ensure_can_edit_user(&app, &session, &metadata.owner).await?;
    let sender = session
        .get::<String>("username")
        .unwrap_or(None)
        .ok_or(UserError::PermissionsError)?;

    let invitation = CollaborationInvite::new(sender.clone(), recipient.clone(), project_id);

    let query = doc! {
        "sender": &sender,
        "recipient": &recipient,
        "projectId": &invitation.project_id
    };
    let update = doc! {
        "$setOnInsert": &invitation
    };
    let options = mongodb::options::UpdateOptions::builder()
        .upsert(true)
        .build();

    let result = app
        .collab_invites
        .update_one(query, update, Some(options))
        .await
        .unwrap();

    // TODO: send via websocket, too
    if result.matched_count == 1 {
        Ok(HttpResponse::Conflict().body("Invitation already exists."))
    } else {  // notify the recipient of the new invitation
    let invitation: api::CollaborationInvite = invitation.into();
    app.network
        .send(network::topology::CollabInviteMsg::new(invitation.clone()))
        .await
        .map_err(InternalError::ActixMessageError)?;

        Ok(HttpResponse::Ok().json(invitation))
    }
}

#[post("/id/{id}")]
async fn respond_to_invite(
    app: web::Data<AppData>,
    state: web::Json<InvitationState>,
    path: web::Path<(String,)>,
    session: Session,
) -> Result<HttpResponse, UserError> {
    let (id,) = path.into_inner();
    let query = doc! {"id": id};

    let invite = app
        .collab_invites
        .find_one(query.clone(), None)
        .await
        .map_err(InternalError::DatabaseConnectionError)?
        .ok_or(UserError::InviteNotFoundError)?;

    ensure_can_edit_user(&app, &session, &invite.receiver).await?;

    app.collab_invites
        .delete_one(query, None)
        .await
        .map_err(InternalError::DatabaseConnectionError)?;

    println!("state: {:?}", state);
    match state.into_inner() {
        InvitationState::ACCEPTED => {
            let query = doc! {"id": &invite.project_id};
            let update = doc! {"$addToSet": {"collaborators": &invite.receiver}};
            let options = FindOneAndUpdateOptions::builder()
                .return_document(ReturnDocument::After)
                .build();

            let updated_metadata = app
                .project_metadata
                .find_one_and_update(query, update, options)
                .await
                .map_err(InternalError::DatabaseConnectionError)?
                .ok_or(UserError::ProjectNotFoundError)?;

            app.on_room_changed(updated_metadata);
            Ok(HttpResponse::Ok().body("Invitation accepted."))
        }
        _ => Ok(HttpResponse::Ok().body("Invitation rejected.")),
    }
}

pub fn config(cfg: &mut web::ServiceConfig) {
    cfg.service(list_invites)
        .service(send_invite)
        .service(respond_to_invite);
}

#[cfg(test)]
mod tests {
    use super::*;
    use actix_web::{test, App, web};
    use netsblox_cloud_common::{User, api};

    use crate::test_utils;


    #[actix_web::test]
    #[ignore]
    async fn test_view_invites() {
        todo!();
    }

    #[actix_web::test]
    #[ignore]
    async fn test_view_invites_403() {
        todo!();
    }

    #[actix_web::test]
    #[ignore]
    async fn test_view_invites_admin() {
        todo!();
    }

    #[actix_web::test]
    async fn test_send_invite() {
        let sender: User = api::NewUser {
            username: "sender".to_string(),
            email: "sender@netsblox.org".into(),
            password: None,
            group_id: None,
            role: None,
        }
        .into();
        let rcvr: User = api::NewUser {
            username: "rcvr".to_string(),
            email: "rcvr@netsblox.org".into(),
            password: None,
            group_id: None,
            role: None,
        }
        .into();

        let project = test_utils::project::builder()
            .with_owner("sender".to_string())
            .build();

        test_utils::setup()
            .with_users(&[sender.clone(), rcvr.clone()])
            .with_projects(&[project.clone()])
            .run(|app_data| async move {
                let app = test::init_service(
                    App::new()
                        .wrap(test_utils::cookie::middleware())
                        .app_data(web::Data::new(app_data.clone()))
                        .configure(config),
                )
                .await;

                let req = test::TestRequest::get()
                    .cookie(test_utils::cookie::new(&sender.username))
                    .uri(&format!("/{}/invite/{}", &project.id, &rcvr.username))
                    .to_request();

                // Ensure that the collaboration invite is returned.
                // This will panic if the response is incorrect so no assert needed.
                let _invite: api::CollaborationInvite = test::call_and_read_body_json(&app, req).await;
            })
            .await;
    }

    #[actix_web::test]
    #[ignore]
    async fn test_send_invite_403() {
        todo!();
    }

    #[actix_web::test]
    #[ignore]
    async fn test_send_invite_admin() {
        todo!();
    }

    #[actix_web::test]
    #[ignore]
    async fn test_respond_to_invite() {
        todo!();
    }

    #[actix_web::test]
    #[ignore]
    async fn test_respond_to_invite_403() {
        todo!();
    }

    #[actix_web::test]
    #[ignore]
    async fn test_respond_to_invite_admin() {
        todo!();
    }

    #[actix_web::test]
    #[ignore]
    async fn test_respond_to_invite_project_deleted() {
        todo!();
    }
}
