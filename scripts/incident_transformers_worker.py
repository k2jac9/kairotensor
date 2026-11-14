"""Fixed versioned worker. Invocation arguments are trusted caller attestations.

Not a public live CLI or security capability. No approval is read from artifacts.
"""
import argparse
import os
import sys
from incident_routing import canonical, _loads
from incident_calibration_process import arm_parent_death
from incident_transformers_backend import Backend, InvocationApproval


def main():
    p=argparse.ArgumentParser()
    p.add_argument('--invocation', choices=('offline_fake_dependencies','local_cached_model'), required=True)
    p.add_argument('--parent-pid',type=int,required=True)
    p.add_argument('--fault',default='none')
    args=p.parse_args();arm_parent_death(args.parent_pid)
    # Fixed choices, never arbitrary modules/executables or an artifact grant.
    factory=None
    if args.invocation=='offline_fake_dependencies':
        from incident_transformers_fake import factory as fake_factory
        factory=lambda:fake_factory('none' if args.fault=='missing_observation' else args.fault)
    elif args.fault!='none':raise ValueError('real invocation cannot inject fixture faults')
    backend=Backend(approval=InvocationApproval(args.invocation),dependency_factory=factory)
    print(canonical(backend.ready()),flush=True)
    for raw in sys.stdin.buffer:
        if len(raw)>65536:raise ValueError('command exceeds wire bound')
        command=_loads(raw)
        result=backend.handle(command)
        if args.invocation=='offline_fake_dependencies' and args.fault=='missing_observation' and command.get('op')=='generate':
            result['observation']=None
        print(canonical(result),flush=True)

if __name__=='__main__':
    try:main()
    except Exception as error:
        print(canonical({'worker_error':f'{type(error).__name__}: {error}'}),flush=True)
        raise SystemExit(1)
