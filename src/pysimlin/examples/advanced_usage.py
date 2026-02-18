#!/usr/bin/env python3
"""Advanced usage examples for the simlin Python package."""

import simlin
import numpy as np
import pandas as pd
from pathlib import Path
from typing import List, Dict, Optional


class ModelAnalyzer:
    """Advanced model analysis utilities."""
    
    def __init__(self, project: simlin.Project):
        self.project = project
        self.model = project.get_model()
        self.var_names = [v.name for v in self.model.variables]
        self._dependency_graph: Optional[Dict[str, List[str]]] = None
    
    def build_dependency_graph(self) -> Dict[str, List[str]]:
        """Build a complete dependency graph of the model."""
        if self._dependency_graph is not None:
            return self._dependency_graph
        
        graph = {}
        for var in self.var_names:
            try:
                deps = self.model.get_incoming_links(var)
                graph[var] = deps
            except simlin.SimlinRuntimeError:
                # Variable might not have dependencies
                graph[var] = []
        
        self._dependency_graph = graph
        return graph
    
    def find_isolated_variables(self) -> List[str]:
        """Find variables with no dependencies and no dependents."""
        graph = self.build_dependency_graph()
        
        # Find variables that depend on nothing
        no_deps = {var for var, deps in graph.items() if not deps}
        
        # Find variables that nothing depends on
        all_deps = set()
        for deps in graph.values():
            all_deps.update(deps)
        
        no_dependents = set(self.var_names) - all_deps
        
        # Isolated variables have neither
        isolated = no_deps & no_dependents
        return sorted(isolated)
    
    def calculate_influence_metrics(self) -> pd.DataFrame:
        """Calculate influence metrics for all variables."""
        graph = self.build_dependency_graph()
        
        metrics = []
        for var in self.var_names:
            # In-degree: number of dependencies
            in_degree = len(graph.get(var, []))
            
            # Out-degree: number of variables that depend on this one
            out_degree = sum(1 for deps in graph.values() if var in deps)
            
            metrics.append({
                'variable': var,
                'in_degree': in_degree,
                'out_degree': out_degree,
                'total_degree': in_degree + out_degree
            })
        
        df = pd.DataFrame(metrics)
        df = df.sort_values('total_degree', ascending=False)
        return df


class SimulationExperiment:
    """Run multiple simulation experiments with different parameters."""
    
    def __init__(self, model: simlin.Model):
        self.model = model
        self.results: List[pd.DataFrame] = []
    
    def run_sensitivity_analysis(
        self, 
        param_name: str,
        param_values: List[float],
        output_vars: List[str]
    ) -> pd.DataFrame:
        """Run sensitivity analysis on a parameter."""
        sensitivity_results = []
        
        for value in param_values:
            # Create new simulation
            sim = self.model.simulate()
            
            # Set parameter value
            try:
                sim.set_value(param_name, value)
            except simlin.SimlinRuntimeError:
                print(f"Warning: Could not set {param_name} = {value}")
                continue
            
            # Run simulation
            sim.run_to_end()
            
            # Collect results
            result = {'param_value': value}
            for var in output_vars:
                try:
                    series = sim.get_series(var)
                    result[f'{var}_mean'] = np.mean(series)
                    result[f'{var}_max'] = np.max(series)
                    result[f'{var}_final'] = series[-1] if len(series) > 0 else np.nan
                except simlin.SimlinRuntimeError:
                    result[f'{var}_mean'] = np.nan
                    result[f'{var}_max'] = np.nan
                    result[f'{var}_final'] = np.nan
            
            sensitivity_results.append(result)
        
        return pd.DataFrame(sensitivity_results)
    
    def run_monte_carlo(
        self,
        param_distributions: Dict[str, tuple],
        output_vars: List[str],
        n_runs: int = 100
    ) -> pd.DataFrame:
        """Run Monte Carlo simulation with parameter distributions."""
        mc_results = []
        
        for run in range(n_runs):
            sim = self.model.simulate()
            
            # Sample and set parameters
            param_values = {}
            for param, (dist_type, *dist_params) in param_distributions.items():
                if dist_type == 'uniform':
                    value = np.random.uniform(dist_params[0], dist_params[1])
                elif dist_type == 'normal':
                    value = np.random.normal(dist_params[0], dist_params[1])
                else:
                    value = dist_params[0]  # Default/constant value
                
                param_values[param] = value
                try:
                    sim.set_value(param, value)
                except simlin.SimlinRuntimeError:
                    pass
            
            # Run simulation
            sim.run_to_end()
            
            # Collect results
            result = {'run': run}
            result.update(param_values)
            
            for var in output_vars:
                try:
                    series = sim.get_series(var)
                    result[f'{var}_mean'] = np.mean(series)
                    result[f'{var}_std'] = np.std(series)
                    result[f'{var}_final'] = series[-1] if len(series) > 0 else np.nan
                except simlin.SimlinRuntimeError:
                    result[f'{var}_mean'] = np.nan
                    result[f'{var}_std'] = np.nan
                    result[f'{var}_final'] = np.nan
            
            mc_results.append(result)
        
        return pd.DataFrame(mc_results)


class LoopDominanceAnalyzer:
    """Analyze loop dominance patterns over time."""
    
    def __init__(self, model: simlin.Model):
        self.model = model
        self.loops = model.get_loops()
    
    def analyze_dominance(self, model: simlin.Model) -> pd.DataFrame:
        """Analyze which loops dominate at different times."""
        if not self.loops:
            return pd.DataFrame()
        
        # Run simulation with LTM
        sim = model.simulate(enable_ltm=True)
        sim.run_to_end()
        
        # Collect loop scores
        loop_data = {}
        for loop in self.loops:
            try:
                scores = sim.get_relative_loop_score(loop.id)
                loop_data[loop.id] = scores
            except simlin.SimlinRuntimeError:
                pass
        
        if not loop_data:
            return pd.DataFrame()
        
        # Create DataFrame
        df = pd.DataFrame(loop_data)
        
        # Add dominance analysis
        df['dominant_loop'] = df.idxmax(axis=1)
        df['max_score'] = df.max(axis=1)
        
        return df


def demonstrate_advanced_features():
    """Demonstrate advanced features of the simlin package."""
    print("Advanced Simlin Usage Examples")
    print("=" * 60)
    
    # Load model
    model_path = Path(__file__).parent / "../tests/fixtures/eval_order.stmx"
    if not model_path.exists():
        print(f"Error: Model not found at {model_path}")
        return

    model = simlin.load(model_path)
    project = model.project
    
    # 1. Model Structure Analysis
    print("\n1. Model Structure Analysis")
    print("-" * 40)
    analyzer = ModelAnalyzer(project)
    
    isolated = analyzer.find_isolated_variables()
    print(f"Isolated variables: {isolated[:5] if isolated else 'None'}")
    
    influence_df = analyzer.calculate_influence_metrics()
    print("\nTop 5 most connected variables:")
    print(influence_df.head())
    
    # 2. Sensitivity Analysis
    print("\n2. Sensitivity Analysis")
    print("-" * 40)
    experiment = SimulationExperiment(model)
    
    var_names = model.get_var_names()
    if len(var_names) >= 2:
        # Pick a parameter and output variable
        param = var_names[0]
        output = var_names[1] if len(var_names) > 1 else var_names[0]
        
        # Note: This might fail if the variable isn't settable
        print(f"Testing sensitivity of {output} to {param}")
        try:
            sensitivity_df = experiment.run_sensitivity_analysis(
                param_name=param,
                param_values=[0.5, 1.0, 1.5, 2.0],
                output_vars=[output]
            )
            if not sensitivity_df.empty:
                print(sensitivity_df)
        except Exception as e:
            print(f"Sensitivity analysis not applicable: {e}")
    
    # 3. Loop Dominance Analysis
    print("\n3. Loop Dominance Analysis")
    print("-" * 40)
    loop_analyzer = LoopDominanceAnalyzer(project)
    
    if loop_analyzer.loops:
        dominance_df = loop_analyzer.analyze_dominance(model)
        if not dominance_df.empty:
            print("Loop dominance over time:")
            print(dominance_df.head())
            
            # Find transitions
            dominant_changes = dominance_df['dominant_loop'].value_counts()
            print(f"\nLoop dominance frequency:")
            print(dominant_changes)
    else:
        print("No loops found in model")
    
    # 4. Batch Processing
    print("\n4. Batch Processing Multiple Models")
    print("-" * 40)
    
    # Example of processing multiple models
    results = []
    for model_name in project.get_model_names()[:3]:  # Process first 3 models
        model = project.get_model(model_name)
        sim = model.simulate()
        sim.run_to_end()
        
        results.append({
            'model': model_name,
            'variables': len(model.variables),
            'steps': sim.get_step_count()
        })
    
    batch_df = pd.DataFrame(results)
    print(batch_df)
    
    print("\n" + "=" * 60)
    print("Advanced examples completed!")


if __name__ == "__main__":
    demonstrate_advanced_features()