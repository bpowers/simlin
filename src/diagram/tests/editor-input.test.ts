/**
 * @jest-environment node
 *
 * Copyright 2025 The Simlin Authors. All rights reserved.
 * Use of this source code is governed by the Apache License,
 * Version 2.0, that can be found in the LICENSE file.
 */

import * as fs from 'fs';
import * as path from 'path';

import { Project as Engine2Project, configureWasm, ready } from '@system-dynamics/engine2';
import { reset } from '@system-dynamics/engine2/internal/wasm';

import type { EditorProps, ProtobufProjectData, JsonProjectData, ProjectData } from '../Editor';

async function loadWasm(): Promise<void> {
  const wasmPath = path.join(__dirname, '..', '..', 'engine2', 'core', 'libsimlin.wasm');
  const wasmBuffer = fs.readFileSync(wasmPath);
  reset();
  configureWasm({ source: wasmBuffer });
  await ready();
}

function loadTestXmile(): Uint8Array {
  const xmilePath = path.join(__dirname, '..', '..', 'pysimlin', 'tests', 'fixtures', 'teacup.stmx');
  if (!fs.existsSync(xmilePath)) {
    throw new Error('Required test XMILE model not found: ' + xmilePath);
  }
  return fs.readFileSync(xmilePath);
}

describe('Editor input format types', () => {
  beforeAll(async () => {
    await loadWasm();
  });

  describe('type definitions', () => {
    it('should accept protobuf format props', () => {
      const mockOnSave = async (_project: ProtobufProjectData, _currVersion: number): Promise<number | undefined> => {
        return 1;
      };

      const props: EditorProps = {
        inputFormat: 'protobuf',
        initialProjectBinary: new Uint8Array([1, 2, 3]),
        initialProjectVersion: 1,
        name: 'test-project',
        onSave: mockOnSave,
      };

      expect(props.inputFormat).toBe('protobuf');
      expect(props.initialProjectBinary).toBeInstanceOf(Uint8Array);
    });

    it('should accept JSON format props', () => {
      const mockOnSave = async (_project: JsonProjectData, _currVersion: number): Promise<number | undefined> => {
        return 1;
      };

      const props: EditorProps = {
        inputFormat: 'json',
        initialProjectJson: '{"name":"test"}',
        initialProjectVersion: 1,
        name: 'test-project',
        onSave: mockOnSave,
      };

      expect(props.inputFormat).toBe('json');
      expect(typeof props.initialProjectJson).toBe('string');
    });
  });

  describe('Project format conversion', () => {
    it('should roundtrip project from XMILE to protobuf and back', async () => {
      const xmileData = loadTestXmile();
      const project = await Engine2Project.open(xmileData);

      const protobuf = project.serializeProtobuf();
      expect(protobuf).toBeInstanceOf(Uint8Array);
      expect(protobuf.length).toBeGreaterThan(0);

      const project2 = await Engine2Project.openProtobuf(protobuf);
      expect(project2.getModelNames()).toEqual(project.getModelNames());

      project.dispose();
      project2.dispose();
    });

    it('should roundtrip project from XMILE to JSON and back', async () => {
      const xmileData = loadTestXmile();
      const project = await Engine2Project.open(xmileData);

      const json = project.serializeJson();
      expect(typeof json).toBe('string');
      expect(json.length).toBeGreaterThan(0);

      const project2 = await Engine2Project.openJson(json);
      expect(project2.getModelNames()).toEqual(project.getModelNames());

      project.dispose();
      project2.dispose();
    });

    it('should produce equivalent projects from protobuf and JSON formats', async () => {
      const xmileData = loadTestXmile();
      const originalProject = await Engine2Project.open(xmileData);

      const protobuf = originalProject.serializeProtobuf();
      const json = originalProject.serializeJson();

      const projectFromProtobuf = await Engine2Project.openProtobuf(protobuf);
      const projectFromJson = await Engine2Project.openJson(json);

      expect(projectFromProtobuf.getModelNames()).toEqual(projectFromJson.getModelNames());

      const protobufVars = projectFromProtobuf.mainModel.variables.map((v) => v.name).sort();
      const jsonVars = projectFromJson.mainModel.variables.map((v) => v.name).sort();
      expect(protobufVars).toEqual(jsonVars);

      originalProject.dispose();
      projectFromProtobuf.dispose();
      projectFromJson.dispose();
    });
  });

  describe('ProjectData discriminated union', () => {
    it('should discriminate ProtobufProjectData by format field', async () => {
      const xmileData = loadTestXmile();
      const project = await Engine2Project.open(xmileData);
      const protobuf = project.serializeProtobuf();

      const data: ProjectData = {
        format: 'protobuf',
        data: protobuf,
      };

      if (data.format === 'protobuf') {
        expect(data.data).toBeInstanceOf(Uint8Array);
        const reopenedProject = await Engine2Project.openProtobuf(data.data as Uint8Array);
        expect(reopenedProject.isSimulatable()).toBe(true);
        reopenedProject.dispose();
      } else {
        throw new Error('Should have discriminated as protobuf');
      }

      project.dispose();
    });

    it('should discriminate JsonProjectData by format field', async () => {
      const xmileData = loadTestXmile();
      const project = await Engine2Project.open(xmileData);
      const json = project.serializeJson();

      const data: ProjectData = {
        format: 'json',
        data: json,
      };

      if (data.format === 'json') {
        expect(typeof data.data).toBe('string');
        const reopenedProject = await Engine2Project.openJson(data.data);
        expect(reopenedProject.isSimulatable()).toBe(true);
        reopenedProject.dispose();
      } else {
        throw new Error('Should have discriminated as json');
      }

      project.dispose();
    });
  });

  describe('serializeForSave equivalent logic', () => {
    it('should return protobuf format when inputFormat is protobuf', async () => {
      const xmileData = loadTestXmile();
      const project = await Engine2Project.open(xmileData);

      const inputFormat: 'protobuf' | 'json' = 'protobuf';
      let result: ProjectData;

      if (inputFormat === 'json') {
        result = { format: 'json', data: project.serializeJson() };
      } else {
        result = { format: 'protobuf', data: project.serializeProtobuf() };
      }

      expect(result.format).toBe('protobuf');
      expect(result.data).toBeInstanceOf(Uint8Array);
      expect((result.data as Uint8Array).length).toBeGreaterThan(0);

      project.dispose();
    });

    it('should return JSON format when inputFormat is json', async () => {
      const xmileData = loadTestXmile();
      const project = await Engine2Project.open(xmileData);

      const inputFormat: 'protobuf' | 'json' = 'json';
      let result: ProjectData;

      if (inputFormat === 'json') {
        result = { format: 'json', data: project.serializeJson() };
      } else {
        result = { format: 'protobuf', data: project.serializeProtobuf() };
      }

      expect(result.format).toBe('json');
      expect(typeof result.data).toBe('string');
      const parsed = JSON.parse(result.data as string);
      expect(parsed).toHaveProperty('models');

      project.dispose();
    });
  });

  describe('openInitialProject equivalent logic', () => {
    it('should open project from protobuf when inputFormat is protobuf', async () => {
      const xmileData = loadTestXmile();
      const originalProject = await Engine2Project.open(xmileData);
      const protobuf = originalProject.serializeProtobuf();
      originalProject.dispose();

      const inputFormat: 'protobuf' | 'json' = 'protobuf';
      let engine2Project: Engine2Project;

      if (inputFormat === 'json') {
        throw new Error('Should not reach here');
      } else {
        engine2Project = await Engine2Project.openProtobuf(protobuf);
      }

      expect(engine2Project).toBeDefined();
      expect(engine2Project.isSimulatable()).toBe(true);
      expect(engine2Project.mainModel.variables.length).toBeGreaterThan(0);

      engine2Project.dispose();
    });

    it('should open project from JSON when inputFormat is json', async () => {
      const xmileData = loadTestXmile();
      const originalProject = await Engine2Project.open(xmileData);
      const json = originalProject.serializeJson();
      originalProject.dispose();

      const inputFormat: 'protobuf' | 'json' = 'json';
      let engine2Project: Engine2Project;

      if (inputFormat === 'json') {
        engine2Project = await Engine2Project.openJson(json);
      } else {
        throw new Error('Should not reach here');
      }

      expect(engine2Project).toBeDefined();
      expect(engine2Project.isSimulatable()).toBe(true);
      expect(engine2Project.mainModel.variables.length).toBeGreaterThan(0);

      engine2Project.dispose();
    });

    it('should throw error for invalid JSON input', async () => {
      const invalidJson = 'not valid json {{{';

      await expect(Engine2Project.openJson(invalidJson)).rejects.toThrow();
    });

    it('should throw error for invalid protobuf input', async () => {
      const invalidProtobuf = new Uint8Array([0, 1, 2, 3, 4, 5, 255, 254, 253]);

      await expect(Engine2Project.openProtobuf(invalidProtobuf)).rejects.toThrow();
    });

    it('should preserve project content when converting between formats', async () => {
      const xmileData = loadTestXmile();
      const originalProject = await Engine2Project.open(xmileData);

      const originalVars = originalProject.mainModel.variables.map((v) => v.name).sort();
      const originalStocks = originalProject.mainModel.stocks.map((s) => s.name).sort();
      const originalFlows = originalProject.mainModel.flows.map((f) => f.name).sort();

      const protobuf = originalProject.serializeProtobuf();
      const json = originalProject.serializeJson();

      const projectFromProtobuf = await Engine2Project.openProtobuf(protobuf);
      const varsFromProtobuf = projectFromProtobuf.mainModel.variables.map((v) => v.name).sort();
      const stocksFromProtobuf = projectFromProtobuf.mainModel.stocks.map((s) => s.name).sort();
      const flowsFromProtobuf = projectFromProtobuf.mainModel.flows.map((f) => f.name).sort();

      expect(varsFromProtobuf).toEqual(originalVars);
      expect(stocksFromProtobuf).toEqual(originalStocks);
      expect(flowsFromProtobuf).toEqual(originalFlows);

      const projectFromJson = await Engine2Project.openJson(json);
      const varsFromJson = projectFromJson.mainModel.variables.map((v) => v.name).sort();
      const stocksFromJson = projectFromJson.mainModel.stocks.map((s) => s.name).sort();
      const flowsFromJson = projectFromJson.mainModel.flows.map((f) => f.name).sort();

      expect(varsFromJson).toEqual(originalVars);
      expect(stocksFromJson).toEqual(originalStocks);
      expect(flowsFromJson).toEqual(originalFlows);

      originalProject.dispose();
      projectFromProtobuf.dispose();
      projectFromJson.dispose();
    });
  });

  describe('onSave callback type safety', () => {
    it('should enforce protobuf callback receives ProtobufProjectData', async () => {
      const xmileData = loadTestXmile();
      const project = await Engine2Project.open(xmileData);
      const protobuf = project.serializeProtobuf();

      const receivedData: ProtobufProjectData[] = [];
      const mockOnSave = async (data: ProtobufProjectData, _currVersion: number): Promise<number | undefined> => {
        receivedData.push(data);
        return 1;
      };

      const projectData: ProtobufProjectData = { format: 'protobuf', data: protobuf };
      await mockOnSave(projectData, 1);

      expect(receivedData.length).toBe(1);
      expect(receivedData[0].format).toBe('protobuf');
      expect(receivedData[0].data).toBeInstanceOf(Uint8Array);

      const reopened = await Engine2Project.openProtobuf(receivedData[0].data as Uint8Array);
      expect(reopened.isSimulatable()).toBe(true);
      reopened.dispose();

      project.dispose();
    });

    it('should enforce JSON callback receives JsonProjectData', async () => {
      const xmileData = loadTestXmile();
      const project = await Engine2Project.open(xmileData);
      const json = project.serializeJson();

      const receivedData: JsonProjectData[] = [];
      const mockOnSave = async (data: JsonProjectData, _currVersion: number): Promise<number | undefined> => {
        receivedData.push(data);
        return 1;
      };

      const projectData: JsonProjectData = { format: 'json', data: json };
      await mockOnSave(projectData, 1);

      expect(receivedData.length).toBe(1);
      expect(receivedData[0].format).toBe('json');
      expect(typeof receivedData[0].data).toBe('string');

      const reopened = await Engine2Project.openJson(receivedData[0].data);
      expect(reopened.isSimulatable()).toBe(true);
      reopened.dispose();

      project.dispose();
    });
  });
});
