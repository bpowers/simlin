// package:
// file: src/schemas/project.proto

import * as jspb from 'google-protobuf';
import * as google_protobuf_timestamp_pb from 'google-protobuf/google/protobuf/timestamp_pb';

export class Project extends jspb.Message {
  getId(): string;
  setId(value: string): void;

  getDisplayName(): string;
  setDisplayName(value: string): void;

  getOwnerId(): string;
  setOwnerId(value: string): void;

  getIsPublic(): boolean;
  setIsPublic(value: boolean): void;

  getDescription(): string;
  setDescription(value: string): void;

  clearTagsList(): void;
  getTagsList(): Array<string>;
  setTagsList(value: Array<string>): void;
  addTags(value: string, index?: number): string;

  clearCollaboratorIdList(): void;
  getCollaboratorIdList(): Array<string>;
  setCollaboratorIdList(value: Array<string>): void;
  addCollaboratorId(value: string, index?: number): string;

  getVersion(): number;
  setVersion(value: number): void;

  getFileId(): string;
  setFileId(value: string): void;

  hasCreated(): boolean;
  clearCreated(): void;
  getCreated(): google_protobuf_timestamp_pb.Timestamp | undefined;
  setCreated(value?: google_protobuf_timestamp_pb.Timestamp): void;

  hasUpdated(): boolean;
  clearUpdated(): void;
  getUpdated(): google_protobuf_timestamp_pb.Timestamp | undefined;
  setUpdated(value?: google_protobuf_timestamp_pb.Timestamp): void;

  serializeBinary(): Uint8Array;
  toObject(includeInstance?: boolean): Project.AsObject;
  static toObject(includeInstance: boolean, msg: Project): Project.AsObject;
  static extensions: { [key: number]: jspb.ExtensionFieldInfo<jspb.Message> };
  static extensionsBinary: { [key: number]: jspb.ExtensionFieldBinaryInfo<jspb.Message> };
  static serializeBinaryToWriter(message: Project, writer: jspb.BinaryWriter): void;
  static deserializeBinary(bytes: Uint8Array): Project;
  static deserializeBinaryFromReader(message: Project, reader: jspb.BinaryReader): Project;
}

export namespace Project {
  export type AsObject = {
    id: string;
    displayName: string;
    ownerId: string;
    isPublic: boolean;
    description: string;
    tagsList: Array<string>;
    collaboratorIdList: Array<string>;
    version: number;
    fileId: string;
    created?: google_protobuf_timestamp_pb.Timestamp.AsObject;
    updated?: google_protobuf_timestamp_pb.Timestamp.AsObject;
  };
}
