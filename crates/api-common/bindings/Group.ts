// This file was generated by [ts-rs](https://github.com/Aleph-Alpha/ts-rs). Do not edit this file manually.
import type { GroupId } from "./GroupId";
import type { ServiceHost } from "./ServiceHost";

export interface Group { id: GroupId, owner: string, name: string, servicesHosts?: Array<ServiceHost>, }