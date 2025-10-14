# Project, Chat, and Memory Architecture

## Background
- The application currently persists projects in Firestore collections accessed through `src/server/models` and exposes them through REST endpoints defined in `src/server/api.ts`.
- Users exist in the data model (`UserPb`, auth middleware, `/user` endpoint), but the product experience assumes a single authenticated user and has no UI or API concepts for switching between users.
- Each project today is a single XMILE model stored as a protobuf (`ProjectPb`) and associated JSON file; there is no notion of multi-document workspaces, chat sessions, or long-lived conversational memory.
- The README positions Simlin as a modeling tool. To support a demoable agent experience we need structured spaces where an agent can collect context (markdown notes) and run multiple conversations per business/policy scenario.

## Goals
- Introduce first-class project workspaces that can be shared across chats and store persistent markdown “memory” owned by the project.
- Support multiple chats per project, where each chat represents a long-running collaboration about a problem.
- Provide an ambient default project so new users can start chatting without learning about project management; they can graduate to explicit projects later.
- Keep the backend simple to operate and avoid data-partition trapdoors (e.g., mixing data from different users or projects).
- Deliver a design concrete enough that an AI agent can implement it incrementally.

## Non-Goals
- Implement real-time collaboration across users (future work).
- Define production authentication/authorization flows beyond user-to-project ownership checks already in the API.
- Optimize for storage or retention policies; we assume Firestore pricing and limits are acceptable for prototype/demo scale.

## High-Level Design

### Entities
1. **User** – existing protobuf object. Owns many projects. No schema changes required beyond adding references to newly created projects, though we will surface project IDs in API responses.
2. **Project** – existing XMILE project plus new metadata:
   - `displayName`, `description`, `isPublic` (existing).
   - New `memoryDocumentIds: string[]` representing ordered markdown memory files.
   - New `defaultChatId` to reference the chat created on first access.
3. **Project Memory Document** – new document type stored in Firestore table `projectMemory`. Fields:
   - `id` (generated `projectId/memoryId`).
   - `projectId` (owner reference, enables queries by prefix).
   - `title` (user visible name like “Market assumptions”).
   - `content` (markdown string).
   - `updatedAt`, `createdAt` timestamps.
   - Optional `pinned` boolean to control UI ordering.
4. **Chat** – new Firestore collection `chats` keyed by `projectId/chatId`.
   - Fields: `id`, `projectId`, `title`, `createdAt`, `updatedAt`, `status` (`open`, `archived`).
   - Each chat owns a collection of messages (see below).
5. **Chat Message** – new Firestore subcollection `chatMessages` (document ID `chatId/messageId`).
   - Fields: `role` (`user`, `assistant`, `system`), `content` (markdown), `modelMetadata` (JSON for temperature, tokens, etc.), `createdAt`.

These entities keep Firestore partition keys aligned (`userId/projectId/...`) to prevent cross-user leakage and simplify security rules.

### Default Project Strategy
- On user creation (existing `populateExamples` flow) or first visit, create an “Ambient Sandbox” project using `createProject` with `isPublic=false`.
- Mark that project with `project.isAmbient=true` so UI/API can highlight it as the default workspace.
- The `/projects` list always returns the ambient project first. Chats started from the landing page implicitly bind to this project.

### Memory Model Rationale
- Storing project memory as markdown documents gives agents long-form context and makes it easy for humans to edit. Documents can be referenced wholesale or chunked at run time for prompting.
- Keeping memory separate from chat transcripts prevents prompt bloat and allows curated summaries.
- The agent should be encouraged to update these documents explicitly via dedicated API calls rather than trying to encode long-term facts in conversation.

### API Changes
All endpoints live under `/api` as today. We extend the router while preserving backward compatibility.

1. **Project Metadata**
   - `GET /api/projects/:username/:projectName` adds `memoryDocumentIds` and `defaultChatId` in the JSON payload.
   - `POST /api/projects/:username/:projectName/memory` creates a new memory document. Body: `{ title, content }`. Returns full document.
   - `PATCH /api/projects/:username/:projectName/memory/:memoryId` updates `title`, `content`, or `pinned`.
   - `GET /api/projects/:username/:projectName/memory` lists memory docs sorted by `pinned desc`, `updatedAt desc`.
2. **Chats**
   - `POST /api/projects/:username/:projectName/chats` creates a chat. Body optional `title`. Returns chat metadata.
   - `GET /api/projects/:username/:projectName/chats` lists chats for that project.
   - `GET /api/projects/:username/:projectName/chats/:chatId/messages` streams paginated messages (query `before`, `limit`).
   - `POST /api/projects/:username/:projectName/chats/:chatId/messages` appends a message. This is how the frontend posts user prompts and stores model replies.
   - `PATCH /api/projects/:username/:projectName/chats/:chatId` for renaming/archiving.
3. **Ambient Project Shortcut**
   - `GET /api/projects/@me/ambient` returns or lazily creates the ambient project and its default chat. This lets the frontend start a chat without selecting a project.

All routes reuse existing authorization guard (`authz.ts`). Only owners (or public viewers for GET) can access memory or chat endpoints.

### Frontend Integration Outline
- Extend project selector UI to display both ambient and explicit projects. Landing page uses ambient automatically.
- Chats view: sidebar shows chat list, main panel shows transcript, right rail shows project memory (editable markdown).
- Memory editor supports markdown preview; agent requests to update memory go through `POST/PATCH` endpoints.

### Agent Workflow Guidance
- When the agent identifies durable facts, it calls memory APIs to append or edit documents.
- Chats remain conversation-specific (e.g., “Evaluate policy scenario A”). Agents can create a new chat for each scenario to keep transcripts scoped.
- Retrieval for prompts should combine (a) latest memory documents, (b) relevant chat history (bounded window), and (c) the project’s XMILE model context.

## Detailed Implementation Plan

### Phase 1: Data Model & Storage
1. **Protobuf updates**: extend `project.proto` with optional `memoryDocumentIds`, `defaultChatId`, `isAmbient`. Regenerate TypeScript outputs via `yarn build:gen-protobufs`.
2. **Firestore Tables**: add `projectMemory`, `chats`, and `chatMessages` to `db-firestore.ts` with typed wrappers in `db-interfaces.ts` and `db.ts` mirroring existing pattern (CRUD functions that accept IDs and protobuf/plain objects).
3. **Memory Document Schema**: implement TypeScript interfaces for memory docs in `src/server/models` and (if needed) create protobuf definitions for storage consistency. For prototype we can store JSON objects without protobufs, but define interfaces to keep compile-time safety.
4. **Indexes/Security**: update Firestore rules (if using) to restrict reads/writes to project owners. Document rule updates alongside new collections.

### Phase 2: Backend API
1. **Ambient Project creation**: update `populateExamples` (and login flow) to call a helper `ensureAmbientProject(user)` that checks for existing ambient project by scanning `project.getIsAmbient()`; if not found, create via `createProject` and store metadata. Return project & chat IDs to caller.
2. **Router additions**: in `src/server/api.ts`, add new routes as described. Use helper functions to resolve `(username, projectName)` to `projectId` and enforce authorization.
3. **Chat service layer**: create `src/server/chat-service.ts` with functions `createChat`, `listChats`, `appendMessage`, `listMessages`, `updateChat`. This isolates Firestore interaction from routing logic and eases testing.
4. **Memory service layer**: similar `src/server/memory-service.ts` for CRUD operations on memory docs.
5. **Message storage**: ensure `appendMessage` writes to `chatMessages` and updates chat `updatedAt`. Consider storing assistant streaming tokens as incremental messages or `partial` flag.
6. **API serialization**: define shared DTOs (TypeScript types) for chat and memory responses to keep client/server aligned.
7. **Tests**: add unit tests for new services (mock DB). Extend existing API integration tests to cover ambient project creation, chat creation, and memory CRUD.

### Phase 3: Frontend & Agent Integration
1. **Shared models**: in `src/app` (or relevant workspace), add TypeScript types mirroring backend DTOs. Ensure fetch wrappers handle the new endpoints.
2. **State management**: extend existing state stores (likely React context or Redux) to track `currentProject`, `chats`, `memoryDocs`, and `activeChatId`.
3. **UI updates**:
   - Project picker: show ambient project labeled “Personal Sandbox”. Allow creating new named projects.
   - Chat workspace: implement layout with chat list, transcript, composer, and memory pane with markdown editor.
   - Memory editor: allow creating/editing documents, saving via API, and highlight pinned docs.
4. **Agent interaction**: define client-side helper for posting user prompts and receiving assistant replies (likely via existing inference infrastructure). After generating a reply, call memory APIs if the agent indicates updates (could use tool-calling or UI button for “save to memory”).
5. **Default path**: landing page automatically fetches `/api/projects/@me/ambient`, sets `currentProject`, and creates a new chat if none open.

### Phase 4: Migration & Deployment
1. **Backfill** existing single-user data: run script to mark the currently stored project as ambient or create a new ambient project populated with default examples.
2. **Data migration**: if introducing protobuf fields, write one-time migration to populate `memoryDocumentIds=[]` for legacy projects.
3. **Monitoring**: add logging (winston) for chat/memory operations to detect failures. Consider quotas for chat length to prevent unbounded storage growth.
4. **Documentation**: update README with instructions for using ambient project and chat UI. Document API endpoints for agent developers.

## Future Considerations
- Real-time collaboration: convert memory docs to shared CRDT or integrate with collaborative editors.
- Advanced retrieval: index memory docs and chats with embeddings for semantic search.
- Permissions: support shared projects and role-based access control beyond owner/public.
- Versioning: implement commit history for memory docs to audit agent changes.

This design keeps user/project boundaries explicit, prevents cross-contamination, and gives agents predictable APIs for long-lived memory while preserving today’s project model and deployment footprint.
