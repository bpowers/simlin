// Copyright 2019 The Model Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { Timestamp } from 'google-protobuf/google/protobuf/timestamp_pb';
import { Set } from 'immutable';
import passport from 'passport';
import { OAuth2Strategy } from 'passport-google-oauth';
import uuidV4 from 'uuid/v4';
import * as logger from 'winston';

import { Application } from './application';
import { Table } from './models/table';
import { User } from './schemas/user_pb';

let AllowedUsers = Set<string>();

async function getOrCreateUserFromProfile(
  users: Table<User>,
  profile: any,
): Promise<[User, undefined] | [undefined, Error]> {
  if (!profile) {
    return [undefined, new Error('no profile returned from Google OAuth2?')];
  }

  if (!profile.emails || !profile.emails.length) {
    const jsonProfile = JSON.stringify(profile);
    return [undefined, new Error(`profile has unexpected shape ${jsonProfile}`)];
  }

  // if we've gotten multiple emails back, just use the main one
  const accountEmail =
    profile.emails.length > 1 ? profile.emails.filter((entry: any) => entry.type === 'account') : profile.emails;
  if (accountEmail.length !== 1) {
    const jsonEmails = JSON.stringify(profile.emails);
    return [undefined, new Error(`expected account email, but not in: ${jsonEmails}`)];
  }

  const email = accountEmail[0].value;
  if (!emailRegExp.test(email)) {
    return [undefined, new Error(`email doesn't look like an email: ${email}`)];
  }

  if (!AllowedUsers.has(email)) {
    return [undefined, new Error(`user not in allowlist`)];
  }

  const displayName = profile.displayName ? profile.displayName : email;

  // we may not be lucky enough to get a photo URL
  let photoUrl: string | undefined;
  if (profile.photos && profile.photos.length && profile.photos[0].value) {
    photoUrl = profile.photos[0].value;
  }

  // since a document with the email already exists, just get the
  // document with it
  let user: User | undefined = await users.findOneByScan({ email });
  if (!user) {
    const created = new Timestamp();
    created.fromDate(new Date());

    user = new User();
    user.setId(`temp-${uuidV4()}`);
    user.setEmail(email);
    user.setDisplayName(displayName);
    user.setProvider('google');
    if (photoUrl) {
      user.setPhotoUrl(photoUrl);
    }
    user.setCreated(created);
    user.setCanCreateProjects(false);

    await users.create(user.getId(), user);
  }

  if (!user) {
    return [undefined, new Error(`unable to insert or find user ${email}`)];
  }

  return [user, undefined];
}

const emailRegExp = /^[^@]+@[^.]+(?:\.[^.]+)+$/;

export const authn = (app: Application): void => {
  const config = app.get('authentication');
  const { google } = config;

  const userAllowlistKey = 'userAllowlist';
  const userAllowlist: string[] = (app.get(userAllowlistKey) || '').split(',');
  if (userAllowlist === undefined || userAllowlist.length === 0) {
    throw new Error(`expected ${userAllowlistKey} in config`);
  }
  AllowedUsers = Set(userAllowlist);

  if (!('MODEL_CLIENT_SECRET' in process.env)) {
    throw new Error('Google OAuth client secret not in environment.');
  }
  google.clientSecret = process.env.MODEL_CLIENT_SECRET;

  const addr = `${app.get('host')}:${app.get('port')}`;
  let callbackURL = `http://${addr}/auth/google/callback`;
  if (process.env.NODE_ENV === 'production') {
    callbackURL = `https://systemdynamics.net/auth/google/callback`;
  }

  passport.use(
    new OAuth2Strategy(
      {
        clientID: google.clientID,
        clientSecret: google.clientSecret,
        callbackURL,
      },
      // eslint-disable-next-line @typescript-eslint/no-misused-promises
      async (accessToken: string, refreshToken: string, profile: any, done: (error: any, user?: any) => void) => {
        const [user, err] = await getOrCreateUserFromProfile(app.db.user, profile);
        if (err !== undefined) {
          logger.error(err);
          done(err);
        } else if (user) {
          done(undefined, user);
        } else {
          throw new Error('unreachable');
        }
      },
    ),
  );

  passport.serializeUser((rawUser: any, done: (error: any, user?: any) => void) => {
    const user: User = rawUser;
    console.log(`serialize user: ${user.getId()}`);
    const serializedUser: any = {
      id: user.getId(),
    };
    done(undefined, serializedUser);
  });

  // eslint-disable-next-line @typescript-eslint/no-misused-promises
  passport.deserializeUser(async (user: any, done: (error: any, user?: any) => void) => {
    if (!user || !user.id) {
      done(new Error(`no or incorrectly serialized User: ${user}`));
      return;
    }

    const userModel = await app.db.user.findOne(user.id);
    if (!userModel) {
      logger.info(`couldn't find user '${user.id}' in DB`);
      done(undefined, null);
      return;
    }
    done(undefined, userModel);
  });

  app.use(passport.initialize());
  app.use(passport.session());

  app.get('/auth/google', passport.authenticate('google', { scope: ['email'] }));

  app.get('/auth/google/callback', passport.authenticate('google', { failureRedirect: '/login' }), (req, resp) => {
    resp.redirect(google.successRedirect);
  });
};
