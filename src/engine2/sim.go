// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

package engine2

import (
	"errors"
	"fmt"
	
	"github.com/tetratelabs/wazero/api"
)

// Sim represents a simulation instance
type Sim struct {
	engine *Engine
	ptr    uint32
}

// NewSim creates a new simulation from a project
func (p *Project) NewSim(modelName string) (*Sim, error) {
	p.engine.mu.Lock()
	defer p.engine.mu.Unlock()
	
	var modelPtr uint32
	var err error
	
	if modelName != "" {
		modelPtr, err = p.engine.writeString(modelName)
		if err != nil {
			return nil, fmt.Errorf("failed to write model name: %w", err)
		}
		defer p.engine.free(modelPtr)
	}
	
	// Call simlin_sim_new
	results, err := p.engine.fnSimNew.Call(p.engine.ctx, uint64(p.ptr), uint64(modelPtr))
	if err != nil {
		return nil, fmt.Errorf("simlin_sim_new failed: %w", err)
	}
	if len(results) != 1 {
		return nil, errors.New("simlin_sim_new returned unexpected number of results")
	}
	
	simPtr := uint32(results[0])
	if simPtr == 0 {
		return nil, errors.New("failed to create simulation")
	}
	
	return &Sim{
		engine: p.engine,
		ptr:    simPtr,
	}, nil
}

// Ref increments the reference count of the simulation
func (s *Sim) Ref() error {
	s.engine.mu.Lock()
	defer s.engine.mu.Unlock()
	
	_, err := s.engine.fnSimRef.Call(s.engine.ctx, uint64(s.ptr))
	return err
}

// Unref decrements the reference count of the simulation
func (s *Sim) Unref() error {
	s.engine.mu.Lock()
	defer s.engine.mu.Unlock()
	
	_, err := s.engine.fnSimUnref.Call(s.engine.ctx, uint64(s.ptr))
	return err
}

// Close releases the simulation resources
func (s *Sim) Close() error {
	return s.Unref()
}

// RunTo runs the simulation to a specified time
func (s *Sim) RunTo(time float64) error {
	s.engine.mu.Lock()
	defer s.engine.mu.Unlock()
	
	results, err := s.engine.fnSimRunTo.Call(s.engine.ctx, uint64(s.ptr), api.EncodeF64(time))
	if err != nil {
		return fmt.Errorf("simlin_sim_run_to failed: %w", err)
	}
	if len(results) != 1 {
		return errors.New("simlin_sim_run_to returned unexpected number of results")
	}
	
	errCode := int32(results[0])
	if errCode != ErrNoError {
		errStr, _ := s.engine.GetErrorString(errCode)
		return fmt.Errorf("simulation failed: %s (code %d)", errStr, errCode)
	}
	
	return nil
}

// RunToEnd runs the simulation to completion
func (s *Sim) RunToEnd() error {
	s.engine.mu.Lock()
	defer s.engine.mu.Unlock()
	
	results, err := s.engine.fnSimRunToEnd.Call(s.engine.ctx, uint64(s.ptr))
	if err != nil {
		return fmt.Errorf("simlin_sim_run_to_end failed: %w", err)
	}
	if len(results) != 1 {
		return errors.New("simlin_sim_run_to_end returned unexpected number of results")
	}
	
	errCode := int32(results[0])
	if errCode != ErrNoError {
		errStr, _ := s.engine.GetErrorString(errCode)
		return fmt.Errorf("simulation failed: %s (code %d)", errStr, errCode)
	}
	
	return nil
}

// Reset resets the simulation to its initial state
func (s *Sim) Reset() error {
	s.engine.mu.Lock()
	defer s.engine.mu.Unlock()
	
	results, err := s.engine.fnSimReset.Call(s.engine.ctx, uint64(s.ptr))
	if err != nil {
		return fmt.Errorf("simlin_sim_reset failed: %w", err)
	}
	if len(results) != 1 {
		return errors.New("simlin_sim_reset returned unexpected number of results")
	}
	
	errCode := int32(results[0])
	if errCode != ErrNoError {
		errStr, _ := s.engine.GetErrorString(errCode)
		return fmt.Errorf("reset failed: %s (code %d)", errStr, errCode)
	}
	
	return nil
}

// GetStepCount returns the number of time steps in the results
func (s *Sim) GetStepCount() (int, error) {
	s.engine.mu.Lock()
	defer s.engine.mu.Unlock()
	
	results, err := s.engine.fnSimGetStepcount.Call(s.engine.ctx, uint64(s.ptr))
	if err != nil {
		return 0, fmt.Errorf("simlin_sim_get_stepcount failed: %w", err)
	}
	if len(results) != 1 {
		return 0, errors.New("simlin_sim_get_stepcount returned unexpected number of results")
	}
	
	count := int32(results[0])
	if count < 0 {
		return 0, errors.New("no results available")
	}
	
	return int(count), nil
}

// GetVarCount returns the number of variables in the model
func (s *Sim) GetVarCount() (int, error) {
	s.engine.mu.Lock()
	defer s.engine.mu.Unlock()
	
	results, err := s.engine.fnSimGetVarcount.Call(s.engine.ctx, uint64(s.ptr))
	if err != nil {
		return 0, fmt.Errorf("simlin_sim_get_varcount failed: %w", err)
	}
	if len(results) != 1 {
		return 0, errors.New("simlin_sim_get_varcount returned unexpected number of results")
	}
	
	count := int32(results[0])
	if count < 0 {
		return 0, errors.New("variable count not available")
	}
	
	return int(count), nil
}

// GetVarNames returns the names of all variables
func (s *Sim) GetVarNames() ([]string, error) {
	s.engine.mu.Lock()
	defer s.engine.mu.Unlock()
	
	// First get the count
	varCount, err := s.GetVarCount()
	if err != nil {
		return nil, err
	}
	
	if varCount == 0 {
		return []string{}, nil
	}
	
	// Allocate array for string pointers
	ptrSize := uint32(varCount * 4) // 4 bytes per pointer
	resultPtr, err := s.engine.malloc(ptrSize)
	if err != nil {
		return nil, fmt.Errorf("failed to allocate result array: %w", err)
	}
	defer s.engine.free(resultPtr)
	
	// Call simlin_sim_get_varnames
	results, err := s.engine.fnSimGetVarnames.Call(s.engine.ctx, uint64(s.ptr), uint64(resultPtr), uint64(varCount))
	if err != nil {
		return nil, fmt.Errorf("simlin_sim_get_varnames failed: %w", err)
	}
	if len(results) != 1 {
		return nil, errors.New("simlin_sim_get_varnames returned unexpected number of results")
	}
	
	count := int32(results[0])
	if count < 0 {
		return nil, errors.New("failed to get variable names")
	}
	
	// Read the string pointers and strings
	var names []string
	for i := 0; i < int(count); i++ {
		// Read string pointer
		ptrOffset := resultPtr + uint32(i*4)
		ptrBytes, ok := s.engine.mod.Memory().Read(ptrOffset, 4)
		if !ok {
			return nil, fmt.Errorf("failed to read string pointer %d", i)
		}
		
		strPtr := uint32(ptrBytes[0]) | uint32(ptrBytes[1])<<8 | uint32(ptrBytes[2])<<16 | uint32(ptrBytes[3])<<24
		name, err := s.engine.readString(strPtr)
		if err != nil {
			return nil, fmt.Errorf("failed to read variable name %d: %w", i, err)
		}
		names = append(names, name)
		
		// Free the string
		s.engine.fnFreeString.Call(s.engine.ctx, uint64(strPtr))
	}
	
	return names, nil
}

// GetValue gets a single value from the simulation
func (s *Sim) GetValue(name string) (float64, error) {
	s.engine.mu.Lock()
	defer s.engine.mu.Unlock()
	
	namePtr, err := s.engine.writeString(name)
	if err != nil {
		return 0, fmt.Errorf("failed to write variable name: %w", err)
	}
	defer s.engine.free(namePtr)
	
	// Allocate space for result
	resultPtr, err := s.engine.malloc(8)
	if err != nil {
		return 0, fmt.Errorf("failed to allocate result: %w", err)
	}
	defer s.engine.free(resultPtr)
	
	// Call simlin_sim_get_value
	results, err := s.engine.fnSimGetValue.Call(s.engine.ctx, uint64(s.ptr), uint64(namePtr), uint64(resultPtr))
	if err != nil {
		return 0, fmt.Errorf("simlin_sim_get_value failed: %w", err)
	}
	if len(results) != 1 {
		return 0, errors.New("simlin_sim_get_value returned unexpected number of results")
	}
	
	errCode := int32(results[0])
	if errCode != ErrNoError {
		errStr, _ := s.engine.GetErrorString(errCode)
		return 0, fmt.Errorf("failed to get value: %s (code %d)", errStr, errCode)
	}
	
	// Read the result
	values, err := s.engine.readFloat64Slice(resultPtr, 1)
	if err != nil {
		return 0, fmt.Errorf("failed to read result: %w", err)
	}
	
	return values[0], nil
}

// SetValue sets a value in the simulation
func (s *Sim) SetValue(name string, value float64) error {
	s.engine.mu.Lock()
	defer s.engine.mu.Unlock()
	
	namePtr, err := s.engine.writeString(name)
	if err != nil {
		return fmt.Errorf("failed to write variable name: %w", err)
	}
	defer s.engine.free(namePtr)
	
	// Call simlin_sim_set_value
	results, err := s.engine.fnSimSetValue.Call(s.engine.ctx, uint64(s.ptr), uint64(namePtr), api.EncodeF64(value))
	if err != nil {
		return fmt.Errorf("simlin_sim_set_value failed: %w", err)
	}
	if len(results) != 1 {
		return errors.New("simlin_sim_set_value returned unexpected number of results")
	}
	
	errCode := int32(results[0])
	if errCode != ErrNoError {
		errStr, _ := s.engine.GetErrorString(errCode)
		return fmt.Errorf("failed to set value: %s (code %d)", errStr, errCode)
	}
	
	return nil
}

// GetSeries gets a time series for a variable
func (s *Sim) GetSeries(name string) ([]float64, error) {
	s.engine.mu.Lock()
	defer s.engine.mu.Unlock()
	
	// Get step count first
	stepCount, err := s.GetStepCount()
	if err != nil {
		return nil, err
	}
	
	if stepCount == 0 {
		return []float64{}, nil
	}
	
	namePtr, err := s.engine.writeString(name)
	if err != nil {
		return nil, fmt.Errorf("failed to write variable name: %w", err)
	}
	defer s.engine.free(namePtr)
	
	// Allocate space for results
	resultSize := uint32(stepCount * 8)
	resultPtr, err := s.engine.malloc(resultSize)
	if err != nil {
		return nil, fmt.Errorf("failed to allocate result array: %w", err)
	}
	defer s.engine.free(resultPtr)
	
	// Call simlin_sim_get_series
	results, err := s.engine.fnSimGetSeries.Call(s.engine.ctx, uint64(s.ptr), uint64(namePtr), uint64(resultPtr), uint64(stepCount))
	if err != nil {
		return nil, fmt.Errorf("simlin_sim_get_series failed: %w", err)
	}
	if len(results) != 1 {
		return nil, errors.New("simlin_sim_get_series returned unexpected number of results")
	}
	
	count := int32(results[0])
	if count < 0 {
		return nil, errors.New("variable not found")
	}
	
	// Read the results
	values, err := s.engine.readFloat64Slice(resultPtr, int(count))
	if err != nil {
		return nil, fmt.Errorf("failed to read series: %w", err)
	}
	
	return values, nil
}

// GetRelLoopScore gets the relative loop score time series for a specific loop
func (s *Sim) GetRelLoopScore(loopID string) ([]float64, error) {
	s.engine.mu.Lock()
	defer s.engine.mu.Unlock()
	
	// Get step count first
	stepCount, err := s.GetStepCount()
	if err != nil {
		return nil, err
	}
	
	if stepCount == 0 {
		return []float64{}, nil
	}
	
	loopIDPtr, err := s.engine.writeString(loopID)
	if err != nil {
		return nil, fmt.Errorf("failed to write loop ID: %w", err)
	}
	defer s.engine.free(loopIDPtr)
	
	// Allocate space for results
	resultSize := uint32(stepCount * 8)
	resultPtr, err := s.engine.malloc(resultSize)
	if err != nil {
		return nil, fmt.Errorf("failed to allocate result array: %w", err)
	}
	defer s.engine.free(resultPtr)
	
	// Call simlin_analyze_get_rel_loop_score
	results, err := s.engine.fnAnalyzeGetRelLoopScore.Call(s.engine.ctx, uint64(s.ptr), uint64(loopIDPtr), uint64(resultPtr), uint64(stepCount))
	if err != nil {
		return nil, fmt.Errorf("simlin_analyze_get_rel_loop_score failed: %w", err)
	}
	if len(results) != 1 {
		return nil, errors.New("simlin_analyze_get_rel_loop_score returned unexpected number of results")
	}
	
	count := int32(results[0])
	if count < 0 {
		return nil, errors.New("loop score not available (LTM may not be enabled)")
	}
	
	// Read the results
	values, err := s.engine.readFloat64Slice(resultPtr, int(count))
	if err != nil {
		return nil, fmt.Errorf("failed to read loop scores: %w", err)
	}
	
	return values, nil
}