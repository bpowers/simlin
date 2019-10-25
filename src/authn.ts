// Copyright 2019 The Model Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { Application } from 'express';
import { Set } from 'immutable';
import * as passport from 'passport';
import { OAuth2Strategy } from 'passport-google-oauth';
import * as logger from 'winston';

import { MongoDuplicateKeyCode } from './models/common';
import { User, UserDocument } from './models/user';

let AllowedUsers = Set<string>();

async function getOrCreateUserFromProfile(profile: any): Promise<[UserDocument, undefined] | [undefined, Error]> {
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

  let user: UserDocument | null = null;
  try {
    // unconditionally try to create to avoid TOCTOU races
    user = await User.create({
      displayName,
      email,
      photoUrl,
      provider: 'google',
    });
  } catch (err) {
    // we expect duplicate key exceptions, anything else is a surprise
    if (err.code !== MongoDuplicateKeyCode) {
      throw err;
    }
    // since a document with the email already exists, just get the
    // document with it
    user = await User.findOne({ email }).exec();
  }

  if (!user) {
    return [undefined, new Error(`unable to insert or find user ${email}`)];
  }

  return [user, undefined];
}

const emailRegExp = /^[^@]+@[^.]+(?:\.[^.]+)+$/;

export const authn = (app: Application): void => {
  const config = app.get('authentication');
  const { google, strategies } = config;

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
      async (accessToken: string, refreshToken: string, profile: any, done: (error: any, user?: any) => void) => {
        const [user, err] = await getOrCreateUserFromProfile(profile);
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

  passport.serializeUser((user: any, done: (error: any, user?: any) => void) => {
    const serializedUser: any = {
      email: user.email,
    };
    if (user.username) {
      serializedUser.username = user.username;
    }
    done(undefined, serializedUser);
  });

  passport.deserializeUser(async (user: any, done: (error: any, user?: any) => void) => {
    if (!user || !user.email) {
      done(new Error(`no or incorrectly serialized User: ${user}`));
      return;
    }

    try {
      const userModel = await User.findOne({ email: user.email }).exec();
      done(undefined, userModel);
    } catch (err) {
      done(err);
    }
  });

  app.use(passport.initialize());
  app.use(passport.session());

  app.get('/auth/google', passport.authenticate('google', { scope: ['email'] }));

  app.get(
    '/auth/google/callback',
    passport.authenticate('google', { failureRedirect: '/login' }),
    async (req, resp) => {
      resp.redirect(google.successRedirect);
    },
  );
};
