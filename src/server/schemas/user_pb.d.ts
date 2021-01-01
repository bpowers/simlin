// package:
// file: src/server/schemas/user.proto

import * as jspb from 'google-protobuf';
import * as google_protobuf_timestamp_pb from 'google-protobuf/google/protobuf/timestamp_pb';

export class User extends jspb.Message {
  getId(): string;
  setId(value: string): void;

  getEmail(): string;
  setEmail(value: string): void;

  getDisplayName(): string;
  setDisplayName(value: string): void;

  getPhotoUrl(): string;
  setPhotoUrl(value: string): void;

  getProvider(): string;
  setProvider(value: string): void;

  hasCreated(): boolean;
  clearCreated(): void;
  getCreated(): google_protobuf_timestamp_pb.Timestamp | undefined;
  setCreated(value?: google_protobuf_timestamp_pb.Timestamp): void;

  getIsAdmin(): boolean;
  setIsAdmin(value: boolean): void;

  getIsDeactivated(): boolean;
  setIsDeactivated(value: boolean): void;

  getCanCreateProjects(): boolean;
  setCanCreateProjects(value: boolean): void;

  serializeBinary(): Uint8Array;
  toObject(includeInstance?: boolean): User.AsObject;
  static toObject(includeInstance: boolean, msg: User): User.AsObject;
  static extensions: { [key: number]: jspb.ExtensionFieldInfo<jspb.Message> };
  static extensionsBinary: { [key: number]: jspb.ExtensionFieldBinaryInfo<jspb.Message> };
  static serializeBinaryToWriter(message: User, writer: jspb.BinaryWriter): void;
  static deserializeBinary(bytes: Uint8Array): User;
  static deserializeBinaryFromReader(message: User, reader: jspb.BinaryReader): User;
}

export namespace User {
  export type AsObject = {
    id: string;
    email: string;
    displayName: string;
    photoUrl: string;
    provider: string;
    created?: google_protobuf_timestamp_pb.Timestamp.AsObject;
    isAdmin: boolean;
    isDeactivated: boolean;
    canCreateProjects: boolean;
  };
}
