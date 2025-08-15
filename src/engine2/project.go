// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

package engine2

import (
	"errors"
	"fmt"
)

// Project represents a simulation project
type Project struct {
	engine *Engine
	ptr    uint32
}

// OpenProject opens a project from protobuf data
func (e *Engine) OpenProject(data []byte) (*Project, error) {
	e.mu.Lock()
	defer e.mu.Unlock()
	
	// Allocate memory for the data
	dataPtr, err := e.writeBytes(data)
	if err != nil {
		return nil, fmt.Errorf("failed to write project data: %w", err)
	}
	defer e.free(dataPtr)
	
	// Allocate memory for the error code
	errPtr, err := e.malloc(4)
	if err != nil {
		return nil, fmt.Errorf("failed to allocate error memory: %w", err)
	}
	defer e.free(errPtr)
	
	// Call simlin_project_open
	results, err := e.fnProjectOpen.Call(e.ctx, uint64(dataPtr), uint64(len(data)), uint64(errPtr))
	if err != nil {
		return nil, fmt.Errorf("simlin_project_open failed: %w", err)
	}
	if len(results) != 1 {
		return nil, errors.New("simlin_project_open returned unexpected number of results")
	}
	
	projectPtr := uint32(results[0])
	if projectPtr == 0 {
		// Read error code
		errBytes, ok := e.mod.Memory().Read(errPtr, 4)
		if ok && len(errBytes) == 4 {
			errCode := int32(errBytes[0]) | int32(errBytes[1])<<8 | int32(errBytes[2])<<16 | int32(errBytes[3])<<24
			errStr, _ := e.GetErrorString(errCode)
			return nil, fmt.Errorf("failed to open project: %s (code %d)", errStr, errCode)
		}
		return nil, errors.New("failed to open project")
	}
	
	return &Project{
		engine: e,
		ptr:    projectPtr,
	}, nil
}

// Ref increments the reference count of the project
func (p *Project) Ref() error {
	p.engine.mu.Lock()
	defer p.engine.mu.Unlock()
	
	_, err := p.engine.fnProjectRef.Call(p.engine.ctx, uint64(p.ptr))
	return err
}

// Unref decrements the reference count of the project
func (p *Project) Unref() error {
	p.engine.mu.Lock()
	defer p.engine.mu.Unlock()
	
	_, err := p.engine.fnProjectUnref.Call(p.engine.ctx, uint64(p.ptr))
	return err
}

// Close releases the project resources
func (p *Project) Close() error {
	return p.Unref()
}

// EnableLTM enables Loops That Matter analysis on the project
func (p *Project) EnableLTM() error {
	p.engine.mu.Lock()
	defer p.engine.mu.Unlock()
	
	results, err := p.engine.fnProjectEnableLTM.Call(p.engine.ctx, uint64(p.ptr))
	if err != nil {
		return fmt.Errorf("simlin_project_enable_ltm failed: %w", err)
	}
	if len(results) != 1 {
		return errors.New("simlin_project_enable_ltm returned unexpected number of results")
	}
	
	errCode := int32(results[0])
	if errCode != ErrNoError {
		errStr, _ := p.engine.GetErrorString(errCode)
		return fmt.Errorf("failed to enable LTM: %s (code %d)", errStr, errCode)
	}
	
	return nil
}

// GetLoops returns all feedback loops in the project
func (p *Project) GetLoops() ([]Loop, error) {
	p.engine.mu.Lock()
	defer p.engine.mu.Unlock()
	
	// Call simlin_analyze_get_loops
	results, err := p.engine.fnAnalyzeGetLoops.Call(p.engine.ctx, uint64(p.ptr))
	if err != nil {
		return nil, fmt.Errorf("simlin_analyze_get_loops failed: %w", err)
	}
	if len(results) != 1 {
		return nil, errors.New("simlin_analyze_get_loops returned unexpected number of results")
	}
	
	loopsPtr := uint32(results[0])
	if loopsPtr == 0 {
		return nil, errors.New("failed to get loops")
	}
	defer func() {
		// Free the loops structure
		p.engine.fnFreeLoops.Call(p.engine.ctx, uint64(loopsPtr))
	}()
	
	// Read the SimlinLoops structure
	// struct SimlinLoops { loops: *mut SimlinLoop, count: usize }
	loopsBytes, ok := p.engine.mod.Memory().Read(loopsPtr, 16) // 8 bytes ptr + 8 bytes count
	if !ok {
		return nil, errors.New("failed to read loops structure")
	}
	
	// Parse the structure (assuming 64-bit pointers)
	loopArrayPtr := uint32(loopsBytes[0]) | uint32(loopsBytes[1])<<8 | uint32(loopsBytes[2])<<16 | uint32(loopsBytes[3])<<24
	loopCount := uint64(loopsBytes[8]) | uint64(loopsBytes[9])<<8 | uint64(loopsBytes[10])<<16 | uint64(loopsBytes[11])<<24 |
		uint64(loopsBytes[12])<<32 | uint64(loopsBytes[13])<<40 | uint64(loopsBytes[14])<<48 | uint64(loopsBytes[15])<<56
	
	if loopCount == 0 {
		return []Loop{}, nil
	}
	
	var loops []Loop
	
	// Read each loop structure
	// struct SimlinLoop { id: *mut c_char, variables: *mut *mut c_char, var_count: usize, polarity: SimlinLoopPolarity }
	loopStructSize := uint32(24) // 8 + 8 + 8 bytes (assuming 64-bit)
	
	for i := uint64(0); i < loopCount; i++ {
		offset := loopArrayPtr + uint32(i)*loopStructSize
		loopBytes, ok := p.engine.mod.Memory().Read(offset, loopStructSize)
		if !ok {
			return nil, fmt.Errorf("failed to read loop %d", i)
		}
		
		// Parse loop structure
		idPtr := uint32(loopBytes[0]) | uint32(loopBytes[1])<<8 | uint32(loopBytes[2])<<16 | uint32(loopBytes[3])<<24
		varsPtr := uint32(loopBytes[8]) | uint32(loopBytes[9])<<8 | uint32(loopBytes[10])<<16 | uint32(loopBytes[11])<<24
		varCount := uint64(loopBytes[16]) | uint64(loopBytes[17])<<8 | uint64(loopBytes[18])<<16 | uint64(loopBytes[19])<<24
		polarity := LoopPolarity(loopBytes[20])
		
		// Read loop ID
		id, err := p.engine.readString(idPtr)
		if err != nil {
			return nil, fmt.Errorf("failed to read loop ID: %w", err)
		}
		
		// Read variable names
		var variables []string
		if varsPtr != 0 && varCount > 0 {
			for j := uint64(0); j < varCount; j++ {
				// Read pointer to string
				ptrOffset := varsPtr + uint32(j)*4
				ptrBytes, ok := p.engine.mod.Memory().Read(ptrOffset, 4)
				if !ok {
					return nil, fmt.Errorf("failed to read variable pointer %d", j)
				}
				
				varPtr := uint32(ptrBytes[0]) | uint32(ptrBytes[1])<<8 | uint32(ptrBytes[2])<<16 | uint32(ptrBytes[3])<<24
				varName, err := p.engine.readString(varPtr)
				if err != nil {
					return nil, fmt.Errorf("failed to read variable name: %w", err)
				}
				variables = append(variables, varName)
			}
		}
		
		loops = append(loops, Loop{
			ID:        id,
			Variables: variables,
			Polarity:  polarity,
		})
	}
	
	return loops, nil
}