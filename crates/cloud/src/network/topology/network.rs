use futures::future::join_all;
use mongodb::bson::{doc, DateTime};
pub use netsblox_core::{BrowserClientState, ClientState, ExternalClientState};
use serde::Serialize;
use std::collections::HashMap;
use std::str::FromStr;
use std::time::{Duration, SystemTime};

use mongodb::Collection;

use crate::models::{ProjectId, ProjectMetadata, SaveState};
use crate::network::topology::address::ClientAddress;
use crate::network::AppID;

pub use super::address::DEFAULT_APP_ID;
use super::client::{Client, ClientID, RoleRequest};
use super::{
    AddClient, BrokenClient, ClientMessage, RemoveClient, SendMessage, SendRoomState,
    SetClientState,
};

struct BrowserAddress {
    role_id: String,
    project_id: String,
}

impl From<BrowserClientState> for BrowserAddress {
    fn from(state: BrowserClientState) -> BrowserAddress {
        BrowserAddress {
            project_id: state.project_id,
            role_id: state.role_id,
        }
    }
}

#[derive(Serialize, Clone)]
struct RoomStateMessage {
    id: String,
    owner: String,
    name: String,
    roles: HashMap<String, RoleState>,
    collaborators: Vec<String>,
    version: u64,
}

#[derive(Serialize, Clone)]
struct RoleState {
    name: String,
    occupants: Vec<OccupantState>,
}

#[derive(Serialize, Clone)]
struct OccupantState {
    id: ClientID,
    name: String,
}

impl RoomStateMessage {
    fn new(
        project: ProjectMetadata,
        room: &ProjectNetwork,
        usernames: &HashMap<ClientID, String>,
    ) -> RoomStateMessage {
        let empty = Vec::new();
        let roles: HashMap<String, RoleState> = project
            .roles
            .into_iter()
            .map(|(id, role)| {
                let client_ids = room.roles.get(&id).unwrap_or(&empty);
                println!("Resolving usernames: {:?} {:?}", client_ids, usernames);
                // TODO: get the names...
                let occupants = client_ids
                    .iter()
                    .map(|id| OccupantState {
                        id: id.to_owned(),
                        name: usernames.get(id).unwrap_or(&"guest".to_owned()).to_owned(),
                    })
                    .collect();

                let state = RoleState {
                    name: role.name,
                    occupants,
                };
                (id, state)
            })
            .collect();

        RoomStateMessage {
            id: room.id.to_owned(),
            owner: project.owner,
            name: project.name,
            roles,
            collaborators: project.collaborators,
            version: SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .expect("Could not get system time")
                .as_secs(),
        }
    }
}

impl From<RoomStateMessage> for ClientMessage {
    fn from(msg: RoomStateMessage) -> ClientMessage {
        let mut value = serde_json::to_value(msg).unwrap();
        let msg = value.as_object_mut().unwrap();
        msg.insert("type".into(), serde_json::to_value("room-roles").unwrap());
        ClientMessage(value)
    }
}

#[derive(Debug)]
struct ProjectNetwork {
    id: String,
    roles: HashMap<String, Vec<ClientID>>,
}

impl ProjectNetwork {
    fn new(id: String) -> ProjectNetwork {
        ProjectNetwork {
            id,
            roles: HashMap::new(),
        }
    }
}

pub struct Topology {
    project_metadata: Option<Collection<ProjectMetadata>>,

    clients: HashMap<ClientID, Client>,
    states: HashMap<ClientID, ClientState>,
    usernames: HashMap<ClientID, String>,

    rooms: HashMap<String, ProjectNetwork>,
    // address_cache: HashMap<String, (String, String)>,
    external: HashMap<AppID, HashMap<String, ClientID>>,
}

#[derive(Debug)]
enum ProjectCleanup {
    NONE,
    IMMEDIATELY,
    DELAYED,
}

impl Topology {
    pub fn new() -> Topology {
        Topology {
            clients: HashMap::new(),
            project_metadata: None,
            rooms: HashMap::new(),
            states: HashMap::new(),
            usernames: HashMap::new(),
            external: HashMap::new(),
        }
    }

    pub fn set_project_metadata(&mut self, project_metadata: Collection<ProjectMetadata>) {
        self.project_metadata = Some(project_metadata);
    }

    async fn get_clients_at(&self, addr: ClientAddress) -> Vec<&Client> {
        let mut client_ids: Vec<&ClientID> = Vec::new();
        let empty = Vec::new();
        for app_id in &addr.app_ids {
            if app_id == DEFAULT_APP_ID {
                let addresses = self.resolve_address(&addr).await;
                let ids = addresses.into_iter().flat_map(|addr| {
                    self.rooms
                        .get(&addr.project_id)
                        .and_then(|room| room.roles.get(&addr.role_id))
                        .unwrap_or(&empty)
                });
                client_ids.extend(ids);
            } else {
                let id = self
                    .external
                    .get(app_id)
                    .map(|network| network.get(&addr.to_app_string()));

                if let Some(id) = id {
                    client_ids.extend(id);
                }
            }
        }

        client_ids
            .into_iter()
            .filter_map(|id| self.clients.get(id))
            .collect()
    }

    async fn resolve_address(&self, addr: &ClientAddress) -> Vec<BrowserAddress> {
        if self.project_metadata.is_none() {
            return Vec::new();
        }

        let project_metadata = self.project_metadata.as_ref().unwrap();

        let mut chunks = addr.address.split('@').rev();
        let project = chunks.next().unwrap();
        let role = chunks.next();

        let query = doc! {"name": project, "owner": &addr.user_id};
        let empty = Vec::new();
        project_metadata
            .find_one(query, None)
            .await
            .unwrap()
            .map(|metadata| {
                let role_names = role.map(|name| vec![name.to_owned()]).unwrap_or_else(|| {
                    metadata
                        .roles
                        .iter()
                        .map(|(_, role)| role.name.to_owned())
                        .collect()
                });

                let name2id = metadata
                    .roles
                    .into_iter()
                    .map(|(k, v)| (v.name, k))
                    .collect::<HashMap<_, _>>();

                role_names
                    .into_iter()
                    .filter_map(|name| name2id.get(&name))
                    .map(|role_id| BrowserAddress {
                        project_id: metadata.id.to_string(),
                        role_id: role_id.to_owned(),
                    })
                    .collect()
            })
            .unwrap_or(empty)
    }

    pub async fn send_msg(&self, msg: SendMessage) {
        let message = ClientMessage(msg.content);
        println!("received message to send to {:?}", msg.addresses);
        let recipients = join_all(
            msg.addresses
                .iter()
                .filter_map(|addr_str| ClientAddress::from_str(addr_str).ok())
                .map(|address| self.get_clients_at(address)),
        )
        .await
        .into_iter()
        .flatten();
        println!("external: {:?}", self.external);
        println!("rooms: {:?}", self.rooms);
        println!("clients: {:?}", self.clients);

        recipients.for_each(|client| {
            println!("Sending msg to client: {}", client.id);
            client.addr.do_send(message.clone()).unwrap();
        });
    }

    fn has_client(&self, id: &str) -> bool {
        self.clients.contains_key(id)
    }

    pub async fn set_client_state(&mut self, msg: SetClientState) {
        if !self.has_client(&msg.id) {
            return;
        }

        println!("Setting client state to {:?}", msg.state);
        self.reset_client_state(&msg.id).await;
        println!("Setting username? {:?}", msg.username);
        if let Some(username) = msg.username {
            self.usernames.insert(msg.id.clone(), username);
        }

        match &msg.state {
            ClientState::Browser(state) => {
                if !self.rooms.contains_key(&state.project_id) {
                    self.rooms.insert(
                        state.project_id.to_owned(),
                        ProjectNetwork::new(state.project_id.to_owned()),
                    );
                }
                let room = self.rooms.get_mut(&state.project_id).unwrap();
                if let Some(occupants) = room.roles.get_mut(&state.role_id) {
                    occupants.push(msg.id.clone());
                } else {
                    room.roles
                        .insert(state.role_id.clone(), vec![msg.id.clone()]);
                }
                let project_id = state.project_id.to_owned();
                self.send_room_state_for(&project_id).await;
            }
            ClientState::External(state) => {
                let app_net = self
                    .external
                    .entry(state.app_id.to_lowercase())
                    .or_insert_with(HashMap::new);

                app_net.insert(state.address.to_owned(), msg.id.to_owned());
            }
        }
        self.states.insert(msg.id, msg.state);
        println!("{:?}", self.states);
    }

    pub fn add_client(&mut self, msg: AddClient) {
        let client = Client::new(msg.id.clone(), msg.addr);
        self.clients.insert(msg.id, client);
    }

    pub async fn set_broken_client(&mut self, msg: BrokenClient) {
        println!("detected broken client!");
        if let Some(project_metadata) = &self.project_metadata {
            if let Some(ClientState::Browser(state)) = self.states.get(&msg.id) {
                let query = doc! {
                    "id": &state.project_id,
                    "saveState": SaveState::TRANSIENT
                };
                let update = doc! {"$set": {"saveState": SaveState::BROKEN}};
                let result = project_metadata
                    .update_one(query, update, None)
                    .await
                    .unwrap();
                println!("Set {} projects broken", result.matched_count);
            }
        }
        // TODO: Record a list of broken clients for the project?
    }

    pub async fn remove_client(&mut self, msg: RemoveClient) {
        self.clients.remove(&msg.id);
        self.reset_client_state(&msg.id).await;
    }

    async fn reset_client_state(&mut self, id: &str) {
        self.usernames.remove(id);
        match self.states.remove(id) {
            Some(ClientState::Browser(state)) => {
                let room = self.rooms.get_mut(&state.project_id);
                let mut empty: Vec<String> = Vec::new();
                let mut update_needed = room.is_some();

                if let Some(room) = room {
                    let occupants = room.roles.get_mut(&state.role_id).unwrap_or(&mut empty);
                    if let Some(pos) = occupants.iter().position(|item| item == id) {
                        occupants.swap_remove(pos);
                    }

                    if occupants.is_empty() {
                        let role_count = room.roles.len();
                        if role_count == 1 {
                            self.remove_room(&state.project_id).await;
                            update_needed = false;
                        } else {
                            // remove the role
                            let room = self.rooms.get_mut(&state.role_id).unwrap();
                            room.roles.remove(&state.role_id);
                        }
                    }
                }

                if update_needed {
                    self.send_room_state_for(&state.project_id).await;
                }
            }
            Some(ClientState::External(state)) => {
                let remove_entry = self
                    .external
                    .get_mut(&state.app_id)
                    .map(|network| {
                        network.remove(&state.address);
                        network.keys().len() == 0
                    })
                    .unwrap_or(false);

                if remove_entry {
                    self.external.remove(&state.app_id);
                }
            }
            None => {}
        }
    }

    async fn remove_room(&mut self, project_id: &str) {
        // TODO: Set the entry to be removed. After how long?
        //   - If the room has only one role, it can be deleted immediately
        //     - the client may need to be updated
        //   - if multiple roles and there is a broken connection:
        //     - delete after an amount of time with no activity - maybe 10 minutes?
        self.rooms.remove(project_id);
        if let Some(project_metadata) = &self.project_metadata {
            // If it has no broken connections, delete it!
            let query = doc! {"id": project_id};
            let cleanup = project_metadata
                .find_one(query.clone(), None)
                .await
                .unwrap()
                .map(|md| match md.save_state {
                    SaveState::TRANSIENT => ProjectCleanup::IMMEDIATELY,
                    SaveState::BROKEN => ProjectCleanup::DELAYED,
                    SaveState::SAVED => ProjectCleanup::NONE,
                })
                .unwrap_or(ProjectCleanup::NONE);

            println!("Project cleanup policy is {:?}", cleanup);
            match cleanup {
                ProjectCleanup::IMMEDIATELY => {
                    project_metadata.delete_one(query, None).await;
                }
                ProjectCleanup::DELAYED => {
                    let ten_minutes = Duration::new(10 * 60, 0);
                    let delete_at = DateTime::from_system_time(
                        SystemTime::now().checked_add(ten_minutes).unwrap(),
                    );
                    let update = doc! {"$set": {"deleteAt": delete_at}};
                    project_metadata.update_one(query, update, None).await;
                }
                _ => {}
            }
        }
    }

    // FIXME: it might be nice not to query the database on *every* occupant invite/move/etc
    // We should be able to cache the addresses since any change should result in a new
    // call to send_room_state
    async fn send_room_state_for(&self, project_id: &ProjectId) {
        if let Some(project_metadata) = &self.project_metadata {
            let query = doc! {"id": project_id};
            if let Some(project) = project_metadata.find_one(query, None).await.unwrap() {
                self.send_room_state(SendRoomState { project });
            }
        }
    }

    pub fn send_room_state(&self, msg: SendRoomState) {
        let id = msg.project.id.to_string();
        if let Some(room) = self.rooms.get(&id) {
            let clients = room
                .roles
                .values()
                .flatten()
                .filter_map(|id| self.clients.get(id));

            let room_state = RoomStateMessage::new(msg.project, room, &self.usernames);
            println!(">>> Sending room update: {}", room_state.name);
            clients.for_each(|client| {
                let _ = client.addr.do_send(room_state.clone().into()); // TODO: handle error?
            });
        }
    }

    pub fn get_role_request(&self, state: BrowserClientState) -> Option<RoleRequest> {
        self.rooms
            .get(&state.project_id)
            .and_then(|room| room.roles.get(&state.role_id))
            .and_then(|client_ids| client_ids.first())
            .and_then(|id| self.clients.get(id))
            .map(|client| RoleRequest::new(client.addr.clone(), state.clone()))
    }
}

#[cfg(test)]
mod tests {
    #[actix_web::test]
    async fn test_remove_client_clear_state() {
        todo!();
    }

    #[actix_web::test]
    async fn test_remove_client_clear_external_state() {
        todo!();
    }

    #[actix_web::test]
    async fn test_filter_blocked_users() {
        todo!();
    }

    #[actix_web::test]
    async fn test_filter_group_msgs() {
        todo!();
    }
    // TODO: Add test for broken connections?
}
