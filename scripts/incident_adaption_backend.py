"""Pure offline Adaption contract. No HTTP, credentials, pricing, or live path.

Schema reference: https://docs.adaptionlabs.ai/api/resources/responses/methods/create
Reviewed 2026-11-14. Provider-reported counts are not verified local tokens, a bill,
a cancellation guarantee, or sufficient authority to release a monetary hold.
"""
import copy
import hashlib

from incident_routing import (FREEZE_SHA256, canonical, load_workload, prepare,
                              validate_output, require, text as require_text, _loads)

VERSION='incident-adaption/1'
METHOD='POST'
PATH='/api/v1/responses'
OUTPUT_CAP=96
MAX_RECEIPT_BYTES=1_048_576
STATUSES={'completed','failed','in_progress','cancelled','queued','incomplete'}


def _model(value):
    require_text(value,'explicit hosted model')
    require(value==value.strip() and not value.startswith('stack:') and len(value)<=256,'explicit hosted model ID required, not a stack')
    require(not any(ord(c)<33 for c in value),'invalid hosted model ID')


def _case(case_id):
    require(type(case_id) is str,'case ID required')
    workload=load_workload()
    matches=[c for c in workload['cases']['cases'] if c['case_id']==case_id and c['split']=='calibration']
    require(len(matches)==1,'only frozen calibration cases are supported')
    return workload,matches[0]


def render_case(case_id, model, states=None):
    """Frozen allowlisted evidence renderer; no labels enter the hosted body.

    states follows incident_routing.prepare's complete calibration-set schema.
    Its eligibility is a fixture/replay attestation, not authenticated Store state.
    """
    _model(model);workload,case=_case(case_id)
    prepared=prepare(workload,'calibration',states)
    record=next(r for r in prepared['requests'] if r['case_id']==case_id)
    require(record['request'] is not None,'evidence_unavailable')
    body={'model':model,'input':_loads(record['request'])['messages'],
          'max_output_tokens':OUTPUT_CAP,'temperature':0,'stream':False,'background':False,
          'tools':[],'tool_choice':'none','agentic_memory':False,'store':False}
    raw=canonical(body).encode('utf-8')
    return {'version':VERSION,'method':METHOD,'path':PATH,'case_id':case_id,'model':model,
            'request_bytes':raw,'request_sha256':hashlib.sha256(raw).hexdigest(),
            'freeze_sha256':FREEZE_SHA256,'evidence_sha256':record['evidence_sha256'],
            'eligibility_authority':prepared['eligibility_authority'],'live_admissible':False}


def _count(value):return type(value) is int and 0<=value<2**63


def _usage(value):
    result={'status':'missing','input_tokens':None,'output_tokens':None,'total_tokens':None,
            'details':{},'raw':copy.deepcopy(value),'issues':[]}
    if value is None:return result
    if type(value) is not dict:
        result.update(status='invalid',issues=['usage must be an object']);return result
    present=[]
    for key in ('input_tokens','output_tokens','total_tokens'):
        if key not in value:continue
        present.append(key)
        if _count(value[key]):result[key]=value[key]
        else:result['issues'].append(f'{key}: nonnegative integer required')
    for key,parent,subtotal in (('input_tokens_details','input_tokens','cached_tokens'),('output_tokens_details','output_tokens','reasoning_tokens')):
        if key not in value:continue
        result['details'][key]=copy.deepcopy(value[key])
        detail=value[key]
        if type(detail) is not dict:result['issues'].append(f'{key}: object required');continue
        if subtotal in detail:
            count=detail[subtotal]
            if not _count(count):result['issues'].append(f'{subtotal}: nonnegative integer required')
            elif result[parent] is not None and count>result[parent]:result['issues'].append(f'{subtotal}: exceeds parent count')
    if all(result[key] is not None for key in ('input_tokens','output_tokens','total_tokens')):
        if result['input_tokens']+result['output_tokens']!=result['total_tokens']:
            result['issues'].append('usage total disagrees with input plus output')
    if result['output_tokens'] is not None and result['output_tokens']>OUTPUT_CAP:
        result['issues'].append('reported output exceeds requested cap')
    result['status']='invalid' if result['issues'] else 'complete' if len(present)==3 else 'partial'
    return result


def parse_response(http_status, raw, *, expected_model, case_id):
    """Classify a previously persisted receipt without semantic gold or pricing.

    Usage remains separately available for refusals/errors. A complete usage
    object alone does not establish response attribution or authorize settlement.
    Missing and invalid counts are never repaired, inferred, or replaced by zero.
    """
    _model(expected_model);_,case=_case(case_id)
    result={'version':VERSION,'disposition':'malformed_response','protocol_valid':False,
            'response_bound':False,'text':None,'validated_output':None,'response_id':None,
            'response_status':None,'usage':_usage(None),'issues':[]}
    try:
        require(type(http_status) is int and 100<=http_status<=599,'invalid HTTP status')
        require(type(raw) is bytes and len(raw)<=MAX_RECEIPT_BYTES,'bounded raw bytes required')
        value=_loads(raw)
        require(type(value) is dict,'response must be an object')
        result['usage']=_usage(value.get('usage'))
        if not 200<=http_status<300:
            result.update(disposition='http_error',issues=['HTTP response was not successful']);return result
        require(type(value.get('id')) is str and bool(value['id']),'response ID required')
        value['id'].encode('utf-8');result['response_id']=value['id']
        require(value.get('object')=='response','response object type')
        require(type(value.get('model')) is str and value['model']==expected_model,'response model mismatch')
        require(type(value.get('status')) is str and value['status'] in STATUSES,'response status')
        result['response_status']=value['status'];result['response_bound']=True
        if value.get('previous_response_id') is not None:
            result['issues'].append('unexpected prior response linkage');result['response_bound']=False
        if 'store' in value and value['store'] is not False:
            result['issues'].append('unexpected response retention');result['response_bound']=False
        if value.get('error') is not None:
            error=value['error'];require(type(error) is dict and type(error.get('code')) is str and type(error.get('message')) is str,'response error shape')
            result['issues'].append('provider reported an error')
        if value.get('incomplete_details') is not None:
            detail=value['incomplete_details'];require(type(detail) is dict and (detail.get('reason') is None or type(detail['reason']) is str),'incomplete detail shape')
            result['issues'].append('provider reported incomplete output')
        require(type(value.get('output')) is list,'output items required')
        require(type(value.get('output_text')) is str,'aggregate output text required')
        value['output_text'].encode('utf-8')
        parts=[];refused=False;unexpected=False;unfinished=False
        for item in value['output']:
            require(type(item) is dict and type(item.get('type')) is str,'output item shape')
            kind=item['type']
            if item.get('status') not in (None,'completed'):unfinished=True
            if kind=='reasoning':continue
            if kind!='message':unexpected=True;continue
            require(item.get('role')=='assistant' and type(item.get('content')) is list,'assistant message required')
            if item.get('status') not in (None,'completed'):unfinished=True
            for part in item['content']:
                require(type(part) is dict and type(part.get('type')) is str,'content part shape')
                if part['type']=='refusal':
                    require(type(part.get('refusal')) is str,'refusal text required');part['refusal'].encode('utf-8');refused=True
                elif part['type']=='output_text':
                    require(part.get('refusal') is None,'ambiguous text/refusal content')
                    require(type(part.get('text')) is str,'text content required');part['text'].encode('utf-8');parts.append(part['text'])
                else:unexpected=True
        answer=''.join(parts)
        require(answer==value['output_text'],'aggregate output text mismatch')
        result['text']=answer
        if value['status']!='completed':
            result['disposition']={'in_progress':'pending','queued':'pending'}.get(value['status'],value['status'])
        elif result['issues'] or unfinished:
            if unfinished:result['issues'].append('message item is not completed')
            result['disposition']='incomplete' if unfinished or value.get('incomplete_details') is not None else 'failed'
        elif unexpected:
            result.update(disposition='unexpected_output',issues=['tool or unknown output despite tools disabled'])
        elif refused:result['disposition']='refusal'
        elif result['usage']['output_tokens'] is not None and result['usage']['output_tokens']>OUTPUT_CAP:
            result.update(disposition='incomplete',issues=['reported output exceeds requested cap'])
        else:
            validated=validate_output(answer,case)
            result.update(disposition='valid_output' if validated is not None else 'invalid_output',
                          protocol_valid=validated is not None,validated_output=validated)
        return result
    except (ValueError,TypeError,KeyError,UnicodeError,OverflowError,RecursionError) as error:
        result.update(disposition='malformed_response',protocol_valid=False,response_bound=False)
        result['issues'].append(str(error));return result
