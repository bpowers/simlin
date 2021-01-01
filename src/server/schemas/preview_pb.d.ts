// package:
// file: src/server/schemas/preview.proto

import * as jspb from 'google-protobuf';
import * as google_protobuf_timestamp_pb from 'google-protobuf/google/protobuf/timestamp_pb';

export class Preview extends jspb.Message {
  getId(): string;
  setId(value: string): void;

  getPng(): Uint8Array | string;
  getPng_asU8(): Uint8Array;
  getPng_asB64(): string;
  setPng(value: Uint8Array | string): void;

  hasCreated(): boolean;
  clearCreated(): void;
  getCreated(): google_protobuf_timestamp_pb.Timestamp | undefined;
  setCreated(value?: google_protobuf_timestamp_pb.Timestamp): void;

  serializeBinary(): Uint8Array;
  toObject(includeInstance?: boolean): Preview.AsObject;
  static toObject(includeInstance: boolean, msg: Preview): Preview.AsObject;
  static extensions: { [key: number]: jspb.ExtensionFieldInfo<jspb.Message> };
  static extensionsBinary: { [key: number]: jspb.ExtensionFieldBinaryInfo<jspb.Message> };
  static serializeBinaryToWriter(message: Preview, writer: jspb.BinaryWriter): void;
  static deserializeBinary(bytes: Uint8Array): Preview;
  static deserializeBinaryFromReader(message: Preview, reader: jspb.BinaryReader): Preview;
}

export namespace Preview {
  export type AsObject = {
    id: string;
    png: Uint8Array | string;
    created?: google_protobuf_timestamp_pb.Timestamp.AsObject;
  };
}
