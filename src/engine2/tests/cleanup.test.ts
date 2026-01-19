// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

jest.mock('../src/internal/analysis', () => ({
  simlin_analyze_get_loops: jest.fn(),
  simlin_free_loops: jest.fn(),
  readLoops: jest.fn(),
  simlin_analyze_get_links: jest.fn(),
  simlin_free_links: jest.fn(),
  readLinks: jest.fn(),
}));

jest.mock('../src/internal/model', () => ({
  simlin_model_unref: jest.fn(),
  simlin_model_get_incoming_links: jest.fn(),
  simlin_model_get_links: jest.fn(),
  simlin_model_get_latex_equation: jest.fn(),
}));

jest.mock('../src/internal/sim', () => ({
  simlin_sim_new: jest.fn(),
  simlin_sim_unref: jest.fn(),
  simlin_sim_run_to: jest.fn(),
  simlin_sim_run_to_end: jest.fn(),
  simlin_sim_reset: jest.fn(),
  simlin_sim_get_stepcount: jest.fn(),
  simlin_sim_get_value: jest.fn(),
  simlin_sim_set_value: jest.fn(),
  simlin_sim_get_series: jest.fn(),
}));

jest.mock('../src/internal/dispose', () => ({
  registerFinalizer: jest.fn(),
  unregisterFinalizer: jest.fn(),
}));

import { Project } from '../src/project';
import { Model } from '../src/model';
import { Sim } from '../src/sim';
import * as analysis from '../src/internal/analysis';
import * as modelInternal from '../src/internal/model';
import * as simInternal from '../src/internal/sim';

const mockedAnalysis = jest.mocked(analysis);
const mockedModel = jest.mocked(modelInternal);
const mockedSim = jest.mocked(simInternal);

describe('cleanup on read errors', () => {
  beforeEach(() => {
    jest.clearAllMocks();
  });

  it('frees loops when Project.getLoops throws during decoding', () => {
    mockedAnalysis.simlin_analyze_get_loops.mockReturnValue(123);
    mockedAnalysis.readLoops.mockImplementation(() => {
      throw new Error('decode failed');
    });

    const ProjectCtor = Project as unknown as new (ptr: number) => Project;
    const project = new ProjectCtor(1);

    expect(() => project.getLoops()).toThrow('decode failed');
    expect(mockedAnalysis.simlin_free_loops).toHaveBeenCalledWith(123);
  });

  it('frees links when Model.getLinks throws during decoding', () => {
    mockedModel.simlin_model_get_links.mockReturnValue(456);
    mockedAnalysis.readLinks.mockImplementation(() => {
      throw new Error('decode failed');
    });

    const model = new Model(1, null, null);

    expect(() => model.getLinks()).toThrow('decode failed');
    expect(mockedAnalysis.simlin_free_links).toHaveBeenCalledWith(456);
  });

  it('frees links when Sim.getLinks throws during decoding', () => {
    mockedSim.simlin_sim_new.mockReturnValue(7);
    mockedAnalysis.simlin_analyze_get_links.mockReturnValue(789);
    mockedAnalysis.readLinks.mockImplementation(() => {
      throw new Error('decode failed');
    });

    const model = new Model(1, null, null);
    const sim = new Sim(model, {}, false);

    expect(() => sim.getLinks()).toThrow('decode failed');
    expect(mockedAnalysis.simlin_free_links).toHaveBeenCalledWith(789);
  });
});
