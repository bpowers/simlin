// package:
// file: src/schemas/file.proto

import * as jspb from 'google-protobuf';
import * as google_protobuf_timestamp_pb from 'google-protobuf/google/protobuf/timestamp_pb';

export class File extends jspb.Message {
  getId(): string;
  setId(value: string): void;

  clearPrevidList(): void;
  getPrevidList(): Array<string>;
  setPrevidList(value: Array<string>): void;
  addPrevid(value: string, index?: number): string;

  getProjectid(): string;
  setProjectid(value: string): void;

  getUserid(): string;
  setUserid(value: string): void;

  hasCreated(): boolean;
  clearCreated(): void;
  getCreated(): google_protobuf_timestamp_pb.Timestamp | undefined;
  setCreated(value?: google_protobuf_timestamp_pb.Timestamp): void;

  getJsoncontents(): string;
  setJsoncontents(value: string): void;

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
    previdList: Array<string>;
    projectid: string;
    userid: string;
    created?: google_protobuf_timestamp_pb.Timestamp.AsObject;
    jsoncontents: string;
  };
}
