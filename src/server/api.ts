// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { Request, Response, Router } from 'express';
import { Timestamp } from 'google-protobuf/google/protobuf/timestamp_pb';
import * as logger from './logger';

import { validateCreateProjectBody, validateSaveProjectBody, validateUserPatchBody } from './api-validation';
import { Application } from './application';
import { Database } from './models/db-interfaces';
import { AlreadyExistsError } from './models/table';
import { populateExamples } from './new-user';
import { createFile, createProject, emptyProject } from './project-creation';
import { renderToPNG } from './render';
import { createDeleteProjectHandler } from './route-handlers';
import { setSessionUser } from './session-auth';
import { Preview as PreviewPb } from './schemas/preview_pb';
import { Project as ProjectPb } from './schemas/project_pb';
import { User as UserPb } from './schemas/user_pb';
import { UsernameDenylist } from './usernames';

function getErrorMessage(error: unknown): string {
  if (error instanceof Error) {
    return error.message;
  }
  if (typeof error === 'string') {
    return error;
  }
  if (typeof error === 'object' && error !== null) {
    const message = (error as Record<string, unknown>).message;
    if (typeof message === 'string') {
      return message;
    }
  }
  return String(error);
}

export async function updatePreview(db: Database, project: ProjectPb): Promise<PreviewPb> {
  const fileDoc = await db.file.findOne(project.getFileId());
  if (!fileDoc) {
    throw new Error(`no File document found for project ${project.getId()}`);
  }

  let png: Uint8Array;
  try {
    png = await renderToPNG(fileDoc);
  } catch (error) {
    throw new Error(`renderToPNG: ${getErrorMessage(error)}`);
  }

  const created = new Timestamp();
  created.fromDate(new Date());

  const preview = new PreviewPb();
  preview.setId(project.getId());
  preview.setPng(png);
  preview.setCreated(created);

  await db.preview.create(preview.getId(), preview);

  return preview;
}

export const maybeGetUser = (req: Request, _res: Response): UserPb | undefined => {
  const user = req.user as unknown as UserPb | undefined;
  if (!user) {
    return undefined;
  }
  return user;
};

export const getUser = (req: Request, res: Response): UserPb => {
  const user = req.user as unknown as UserPb | undefined;
  if (!user) {
    // Reachable only if authz's public carve-out admits a path whose
    // handler assumes authentication -- the carve-out pattern in authz.ts
    // is kept aligned with the router's dispatch to prevent exactly that
    // -- so this is defense in depth, answering with the same {error}
    // envelope as the rest of the API.
    logger.warn(`user not found, but passed authz?`);
    res.status(500).json({ error: 'internal error' });
    throw new Error(`user not found, but passed authz?`);
  }
  return user;
};

export const apiRouter = (app: Application): Router => {
  const api = Router();

  api.get('/user', (req: Request, res: Response): void => {
    const user = getUser(req, res);
    res.status(200).json(user.toObject());
  });

  // create a new project
  api.post('/projects', async (req: Request, res: Response): Promise<void> => {
    const user = getUser(req, res);

    const createBodyError = validateCreateProjectBody(req.body);
    if (createBodyError) {
      res.status(400).json({ error: createBodyError });
      return;
    }

    const projectName = (req.body.projectName as string) || '';
    const projectDescription = (req.body.description as string) || '';
    const isPublic = !!req.body.isPublic;

    try {
      const project = createProject(user, projectName, projectDescription, isPublic);

      // Fast-fail before any write: a taken name is the common create
      // failure, and detecting it only at project.create below would leave
      // the just-written File orphaned with no project doc to reap it
      // through. Concurrent creates of the same name can still slip past
      // this read; project.create stays the source of truth and the loser
      // cleans up its File.
      if (await app.db.project.findOne(project.getId())) {
        res.status(400).json({ error: 'project name already taken' });
        return;
      }

      let sdPB: Buffer | undefined;
      if (req.body.projectPB) {
        sdPB = Buffer.from(req.body.projectPB, 'base64');
      } else {
        sdPB = Buffer.from(await emptyProject(projectName, user.getDisplayName()));
      }

      const filePb = createFile(project.getId(), user.getId(), undefined, sdPB);

      // File ids hash content plus creation-millisecond, so a duplicate-id
      // rejection means a byte-identical doc already exists; reusing it is
      // sound because the two are interchangeable by construction.
      let createdFile = true;
      try {
        await app.db.file.create(filePb.getId(), filePb);
      } catch (err) {
        if (!(err instanceof AlreadyExistsError)) {
          throw err;
        }
        createdFile = false;
      }

      project.setFileId(filePb.getId());
      try {
        await app.db.project.create(project.getId(), project);
      } catch (err) {
        // Same cleanup rule as the save path's 409 branch: delete only a
        // File this request actually wrote, and never one the winning row
        // references (an identical concurrent create in the same
        // millisecond shares our file id). A crash before this delete
        // leaks the File PERMANENTLY -- unlike save-path leaks, which are
        // at least reaped when their project is deleted, this File's
        // project row was never created, so nothing scans for it (unless
        // a same-named project is later created and then deleted).
        // Closing the window needs a cross-table transaction the Table
        // interface doesn't expose.
        if (createdFile) {
          try {
            const winner = await app.db.project.findOne(project.getId());
            if (winner?.getFileId() !== filePb.getId()) {
              await app.db.file.deleteOne(filePb.getId());
            }
          } catch (cleanupErr) {
            logger.warn(`unable to delete orphaned file ${filePb.getId()} for ${project.getId()}: ${cleanupErr}`);
          }
        }
        if (err instanceof AlreadyExistsError) {
          res.status(400).json({ error: 'project name already taken' });
          return;
        }
        throw err;
      }

      res.status(200).json(project.toObject());
    } catch (error) {
      logger.error(`POST /projects (${user.getId()}, ${projectName}): ${getErrorMessage(error)}`);
      logger.error(error);
      throw error;
    }
  });

  api.get('/projects', async (req: Request, res: Response): Promise<void> => {
    const user = getUser(req, res);
    const projectModels = await app.db.project.find(user.getId() + '/');
    const projects = await Promise.all(projectModels.map((project: ProjectPb) => project.toObject()));
    res.status(200).json(projects);
  });

  api.get('/projects/:username/:projectName', async (req: Request, res: Response): Promise<void> => {
    const requestUser = maybeGetUser(req, res);
    // avoid doing 2 DB queries to look up the same user, if the
    // author is the one making this request
    let authorUser: UserPb | undefined;
    if (requestUser && requestUser.getId() === req.params.username) {
      authorUser = requestUser;
    } else {
      authorUser = await app.db.user.findOne(req.params.username as string);
    }
    if (!authorUser) {
      res.status(404).json({});
      return;
    }

    const projectSlug = `${req.params.username}/${req.params.projectName}`;
    const projectModel = await app.db.project.findOne(projectSlug);

    // the username check is skipped if the model exists and is public
    if (!projectModel?.getIsPublic()) {
      // TODO: implement collaborators
      if (requestUser?.getId() !== authorUser.getId()) {
        res.status(401).json({ error: 'unauthorized' });
        return;
      }
    }

    if (!projectModel || !projectModel.getFileId()) {
      res.status(404).json({});
      return;
    }

    const file = await app.db.file.findOne(projectModel.getFileId());
    if (!file) {
      res.status(404).json({});
      return;
    }

    const project = projectModel.toObject();
    const jsonFile = file.getJsonContents();
    const pb = file.getProjectContents_asB64();

    res.status(200).json({ ...project, file: jsonFile, pb });
  });

  api.get('/preview/:username/:projectName', async (req: Request, res: Response): Promise<void> => {
    const requestUser = getUser(req, res);
    // avoid doing 2 DB queries to look up the same user, if the
    // author is the one making this request
    let authorUser: UserPb | undefined = requestUser;
    if (requestUser.getId() !== req.params.username) {
      authorUser = await app.db.user.findOne(req.params.username as string);
    }
    if (!authorUser) {
      res.status(404).json({});
      return;
    }

    const projectSlug = `${req.params.username}/${req.params.projectName}`;
    const projectModel = await app.db.project.findOne(projectSlug);

    // the username check is skipped if the model exists and is public
    if (!projectModel?.getIsPublic()) {
      // TODO: implement collaborators
      if (requestUser.getId() !== authorUser.getId()) {
        res.status(401).json({ error: 'unauthorized' });
        return;
      }
    }

    if (!projectModel || !projectModel.getFileId()) {
      res.status(404).json({});
      return;
    }

    let previewModel = await app.db.preview.findOne(projectSlug);
    if (!previewModel) {
      try {
        previewModel = await updatePreview(app.db, projectModel);
      } catch (err) {
        logger.error(`updatePreview: ${err}`);
        res.status(500).json({ error: 'unable to render preview' });
        return;
      }
    }

    const png = Buffer.from(previewModel.getPng() as Uint8Array);

    res.contentType('image/png');
    res.status(200).send(png);
  });

  api.post('/projects/:username/:projectName', async (req: Request, res: Response): Promise<void> => {
    const user = getUser(req, res);
    // TODO
    if (user.getId() !== req.params.username) {
      res.status(401).json({ error: 'unauthorized' });
      return;
    }
    const projectSlug = `${req.params.username}/${req.params.projectName}`;
    const projectModel = await app.db.project.findOne(projectSlug);
    if (!projectModel || !projectModel.getFileId()) {
      res.status(404).json({});
      return;
    }

    const saveBodyError = validateSaveProjectBody(req.body);
    if (saveBodyError) {
      res.status(400).json({ error: saveBodyError });
      return;
    }

    const projectVersion = req.body.currVersion as number;
    const newVersion = projectVersion + 1;
    const staleVersionError = `error saving model: changes based on old version. refresh page to reload`;

    // Fast-fail before persisting anything: without this, every save from a
    // stale tab wrote a File document that nothing would ever point at. Two
    // concurrent saves can still both pass this read-then-compare; the
    // conditional update below stays the source of truth for that race.
    if (projectModel.getVersion() !== projectVersion) {
      res.status(409).json({ error: staleVersionError });
      return;
    }

    const pbContents = Buffer.from(req.body.projectPB as string, 'base64');

    // The project row must reference an already-persisted File, and the
    // Table interface offers no cross-table transaction, so the File is
    // written first and only becomes reachable if the conditional update
    // succeeds; the loser of a version race cleans up below. The remaining
    // orphan window is a crash between these two writes.
    const file = createFile(projectModel.getId(), user.getId(), undefined, pbContents);

    // File ids hash content plus creation-millisecond, so a duplicate-id
    // rejection means a byte-identical doc already exists (an identical
    // save landed in the same millisecond); reusing it is sound because
    // the two are interchangeable by construction.
    let createdFile = true;
    try {
      await app.db.file.create(file.getId(), file);
    } catch (err) {
      if (!(err instanceof AlreadyExistsError)) {
        throw err;
      }
      createdFile = false;
    }

    // Two-guard orphan cleanup, shared by the conflict (null) and
    // transport-failure (throw) outcomes of the conditional update: never
    // delete a doc another request wrote (createdFile), and never delete
    // the doc the row now references -- a concurrent winner that saved
    // identical content in the same millisecond carries the SAME file id.
    // Residual windows: a crash before the delete leaks one File, and a
    // same-id winner committing between the re-read and the delete could
    // lose one; both need a cross-table transaction the Table interface
    // doesn't expose, and both require identical same-millisecond saves.
    const cleanupUnreferencedFile = async (): Promise<void> => {
      if (!createdFile) {
        return;
      }
      try {
        const winner = await app.db.project.findOne(projectSlug);
        if (winner?.getFileId() !== file.getId()) {
          await app.db.file.deleteOne(file.getId());
        }
      } catch (err) {
        logger.warn(`unable to delete orphaned file ${file.getId()} for ${projectSlug}: ${err}`);
      }
    };

    // only update if the version matches
    projectModel.setFileId(file.getId());
    projectModel.setVersion(newVersion);

    let result: ProjectPb | null;
    try {
      result = await app.db.project.update(
        projectModel.getId(),
        {
          version: projectVersion,
        },
        projectModel,
      );
    } catch (err) {
      // A rejected update is a transport failure, not a version conflict
      // (the table maps precondition misses to null): clean up and let it
      // surface as a 500 rather than steering the client into its
      // destructive conflict-recovery flow.
      await cleanupUnreferencedFile();
      throw err;
    }

    // if the result is null we weren't able to find a matching
    // version in the DB, probably due to concurrent modification in
    // a different browser tab
    if (result === null) {
      await cleanupUnreferencedFile();
      res.status(409).json({ error: staleVersionError });
      return;
    }

    // The cached preview renders the file the project row points at, so it
    // only goes stale when the row actually changed; failed saves above
    // must leave it in place.
    setTimeout(async () => {
      try {
        await app.db.preview.deleteOne(projectModel.getId());
      } catch {
        logger.warn(`unable to delete preview for ${req.params.projectName}`);
      }
    });

    res.status(200).json({ version: newVersion });
  });

  api.delete('/projects/:username/:projectName', createDeleteProjectHandler({ db: app.db }));

  api.patch('/user', async (req: Request, res: Response): Promise<void> => {
    const userModel = getUser(req, res);

    const patchBodyError = validateUserPatchBody(req.body);
    if (patchBodyError) {
      res.status(400).json({ error: patchBodyError });
      return;
    }

    if (!req.body.agreeToTermsAndPrivacyPolicy) {
      res.status(400).json({ error: 'must agree to Terms and Conditions and Privacy Policy' });
      return;
    }

    const proposedUsername = req.body.username as string;

    if (UsernameDenylist.has(proposedUsername)) {
      res.status(400).json({ error: 'username already taken' });
      return;
    }

    if (!userModel.getId().startsWith(`temp-`)) {
      res.status(403).json({ error: 'username already set' });
      return;
    }

    const origUserId = userModel.getId();

    userModel.setId(proposedUsername);
    userModel.setCanCreateProjects(true);
    try {
      // updating the primary key of a user doesn't work in mongo
      logger.error(`creating user ${userModel.getId()}`);
      await app.db.user.create(userModel.getId(), userModel);
      logger.error(`deleting old user ${origUserId}`);
      await app.db.user.deleteOne(origUserId);
      logger.error(`done deleting old user ${origUserId}`);
    } catch (error) {
      if (error instanceof AlreadyExistsError) {
        res.status(400).json({ error: 'username already taken' });
        return;
      }
      // Anything else here (including the deleteOne after a successful
      // create) is not "name taken"; reporting it as such told users to
      // pick a different name when retrying the same one was fine.
      logger.error(`PATCH /user rename ${origUserId} -> ${userModel.getId()}: ${getErrorMessage(error)}`);
      res.status(500).json({ error: 'internal error' });
      return;
    }

    // re-key the session to the user's chosen id (their old temp- record
    // is gone, so the pre-rename cookie would otherwise go stale)
    setSessionUser(req, userModel.getId());

    const defaultProjectsDir = app.get('defaultProjectsDir') as string;
    // this error shouldn't ever happen, but also shouldn't be fatal
    if (defaultProjectsDir) {
      try {
        await populateExamples(app.db, userModel, defaultProjectsDir);
      } catch (err) {
        logger.error(`populateExamples(${userModel.getId()}, ${defaultProjectsDir}): ${err}`);
      }
    } else {
      logger.error('missing defaultProjectsDir in config');
    }

    res.status(200).json({});
  });

  return api;
};
