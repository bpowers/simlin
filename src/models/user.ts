// Copyright 2019 The Model Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { Document, Model, model, Schema } from 'mongoose';

export type Provider = 'google';

const userKeysAllowlist = new Set(['displayName', 'email', 'username', 'photoUrl']);

interface UserModel {
  displayName: string;
  email: string;
  username: string | undefined;
  photoUrl: string | undefined;
  provider: Provider;
  isAdmin: boolean;
  isDeactivated: boolean;
}

const UserSchema: Schema = new Schema({
  displayName: String,
  email: {
    type: String,
    required: true,
    unique: true,
  },
  username: {
    type: String,
    unique: true,
  },
  photoUrl: String,
  provider: {
    type: String,
    required: true,
  },
  isAdmin: Boolean,
  isDeactivated: Boolean,
});
UserSchema.set('toJSON', {
  transform: (doc: any, ret: any, options: any): any => {
    const allKeys = Object.keys(ret);
    const toRemove = allKeys.filter((key: string) => !userKeysAllowlist.has(key));
    for (const key of toRemove) {
      delete ret[key];
    }
    return ret;
  },
});

export interface UserDocument extends UserModel, Document {}

export const User: Model<UserDocument> = model<UserDocument>('User', UserSchema);
