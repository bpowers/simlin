// package:
// file: src/schemas/file.proto

import * as jspb from 'google-protobuf';
import * as google_protobuf_timestamp_pb from 'google-protobuf/google/protobuf/timestamp_pb';

export class File extends jspb.Message {
  getId(): string;
  setId(value: string): void;

  clearPrevIdList(): void;
  getPrevIdList(): Array<string>;
  setPrevIdList(value: Array<string>): void;
  addPrevId(value: string, index?: number): string;

  getProjectId(): string;
  setProjectId(value: string): void;

  getUserId(): string;
  setUserId(value: string): void;

  hasCreated(): boolean;
  clearCreated(): void;
  getCreated(): google_protobuf_timestamp_pb.Timestamp | undefined;
  setCreated(value?: google_protobuf_timestamp_pb.Timestamp): void;

  getJsonContents(): string;
  setJsonContents(value: string): void;

  getProjectContents(): Uint8Array | string;
  getProjectContents_asU8(): Uint8Array;
  getProjectContents_asB64(): string;
  setProjectContents(value: Uint8Array | string): void;

  serializeBinary(): Uint8Array;
  toObject(includeInstance?: boolean): File.AsObject;
  static toObject(includeInstance: boolean, msg: File): File.AsObject;
  static extensions: { [key: number]: jspb.ExtensionFieldInfo<jspb.Message> };
  static extensionsBinary: { [key: number]: jspb.ExtensionFieldBinaryInfo<jspb.Message> };
  static serializeBinaryToWriter(message: File, writer: jspb.BinaryWriter): void;
  static deserializeBinary(bytes: Uint8Array): File;
  static deserializeBinaryFromReader(message: File, reader: jspb.BinaryReader): File;
}

export namespace File {
  export type AsObject = {
    id: string;
    prevIdList: Array<string>;
    projectId: string;
    userId: string;
    created?: google_protobuf_timestamp_pb.Timestamp.AsObject;
    jsonContents: string;
    projectContents: Uint8Array | string;
  };
}
