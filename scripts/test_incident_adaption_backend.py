import copy
import json
import unittest
from incident_adaption_backend import render_case, parse_response
from incident_routing import canonical, load_workload, prepare

MODEL='fixture-hosted-model/1'
TEXT='{"decision":"abstain","owner":null,"reason":"undetermined_cause","evidence_ids":["e01"]}'

def response(**changes):
    value={'id':'resp-fixture-1','object':'response','status':'completed','model':MODEL,
           'output':[{'type':'message','role':'assistant','status':'completed','content':[{'type':'output_text','text':TEXT}]}],
           'output_text':TEXT,'usage':{'input_tokens':101,'output_tokens':12,'total_tokens':113}}
    value.update(changes);return value

def parse(value,status=200):return parse_response(status,canonical(value).encode(),expected_model=MODEL,case_id='cal-01')

class AdaptionTests(unittest.TestCase):
    def test_frozen_renderer_explicit_controls_and_no_labels(self):
        r=render_case('cal-01',MODEL);body=json.loads(r['request_bytes'])
        self.assertEqual(body['input'],json.loads(prepare(load_workload())['requests'][0]['request'])['messages'])
        self.assertEqual(set(body),{'model','input','max_output_tokens','temperature','stream','background','tools','tool_choice','agentic_memory','store'})
        self.assertEqual(body['max_output_tokens'],96);self.assertEqual(body['temperature'],0)
        for key in ('stream','background','agentic_memory','store'):self.assertIs(body[key],False)
        self.assertEqual(body['tools'],[]);self.assertEqual(body['tool_choice'],'none')
        self.assertNotIn(b'case_id',r['request_bytes']);self.assertNotIn(b'expected',r['request_bytes'])
        self.assertEqual(r,render_case('cal-01',MODEL));self.assertEqual(r['path'],'/api/v1/responses')
    def test_renderer_rejects_model_ambiguity_and_heldout(self):
        for model in ('',' ','stack:test',None,True):
            with self.subTest(model=model),self.assertRaises(ValueError):render_case('cal-01',model)
        with self.assertRaises(ValueError):render_case('hold-01',MODEL)
    def test_evidence_missing_stale_refused_no_silent_filter(self):
        states=prepare(load_workload())['states']
        for state in ('missing','stale','refused'):
            changed=copy.deepcopy(states);changed[0]['evidence'][0]['state']=state
            with self.subTest(state=state),self.assertRaises(ValueError):render_case('cal-01',MODEL,changed)
    def test_success_and_provider_usage_not_semantic_gold(self):
        r=parse(response());self.assertTrue(r['protocol_valid']);self.assertTrue(r['response_bound'])
        self.assertEqual(r['disposition'],'valid_output');self.assertEqual(r['usage']['status'],'complete')
        self.assertEqual(r['usage']['total_tokens'],113)
    def test_malformed_output_is_measured_but_not_valid(self):
        r=parse(response(output_text='oops',output=[{'type':'message','role':'assistant','content':[{'type':'output_text','text':'oops'}]}]))
        self.assertEqual(r['disposition'],'invalid_output');self.assertFalse(r['protocol_valid'])
        self.assertEqual(r['usage']['status'],'complete')
    def test_refusal_incomplete_failure_pending_and_cancelled(self):
        r=parse(response(output_text='',output=[{'type':'message','role':'assistant','content':[{'type':'refusal','refusal':'no'}]}]))
        self.assertEqual(r['disposition'],'refusal')
        for status in ('incomplete','failed','in_progress','queued','cancelled'):
            r=parse(response(status=status,incomplete_details={'reason':'max_output_tokens'} if status=='incomplete' else None))
            self.assertFalse(r['protocol_valid']);self.assertNotEqual(r['disposition'],'valid_output')
            self.assertEqual(r['usage']['total_tokens'],113)
    def test_contradictory_text_tools_and_wrong_model_refused(self):
        for value in (response(output_text='different'),response(model='other'),response(output=[{'type':'function_call','name':'x'}])):
            r=parse(value);self.assertFalse(r['protocol_valid']);self.assertTrue(r['issues'])
        self.assertFalse(parse(response(model='other'))['response_bound'])
    def test_partial_and_missing_usage_not_zero(self):
        for usage,status in ((None,'missing'),({},'partial'),({'input_tokens':10},'partial')):
            r=parse(response(usage=usage));self.assertEqual(r['usage']['status'],status)
            self.assertIsNone(r['usage']['output_tokens']);self.assertIsNone(r['usage']['total_tokens'])
    def test_bad_usage_is_invalid_and_retained(self):
        for usage in ({'input_tokens':True,'output_tokens':2,'total_tokens':3},
                      {'input_tokens':-1,'output_tokens':2,'total_tokens':1},
                      {'input_tokens':1.0,'output_tokens':2,'total_tokens':3},
                      {'input_tokens':2**63,'output_tokens':2,'total_tokens':2**63+2},
                      {'input_tokens':1,'output_tokens':2,'total_tokens':4}):
            r=parse(response(usage=usage));self.assertEqual(r['usage']['status'],'invalid');self.assertEqual(r['usage']['raw'],usage)
    def test_details_retained_not_added_to_totals(self):
        usage={'input_tokens':101,'output_tokens':12,'total_tokens':113,'input_tokens_details':{'cached_tokens':50},'output_tokens_details':{'reasoning_tokens':9}}
        r=parse(response(usage=usage));self.assertEqual(r['usage']['total_tokens'],113)
        self.assertEqual(r['usage']['details']['output_tokens_details'],{'reasoning_tokens':9})
        for bad in (True,-1,13,'9'):
            usage['output_tokens_details']['reasoning_tokens']=bad
            self.assertEqual(parse(response(usage=usage))['usage']['status'],'invalid')
    def test_duplicate_keys_bad_utf8_deep_json_and_http_error(self):
        for raw in (b'{"id":1,"id":2}',b'\xff',b'['*100+b'0'+b']'*100,b'not json'):
            r=parse_response(200,raw,expected_model=MODEL,case_id='cal-01')
            self.assertEqual(r['disposition'],'malformed_response');self.assertFalse(r['protocol_valid'])
        r=parse(response(),500);self.assertEqual(r['disposition'],'http_error');self.assertFalse(r['response_bound'])
    def test_response_constraints_and_cap_violation(self):
        for change in ({'store':True},{'previous_response_id':'old'},{'error':{'code':'x','message':'bad'}},{'usage':{'input_tokens':1,'output_tokens':97,'total_tokens':98}}):
            r=parse(response(**change));self.assertFalse(r['protocol_valid']);self.assertTrue(r['issues'])
    def test_reasoning_separate_from_answer(self):
        value=response();value['output'].insert(0,{'type':'reasoning','summary':[{'type':'summary_text','text':'not answer'}]})
        r=parse(value);self.assertTrue(r['protocol_valid']);self.assertEqual(r['text'],TEXT)
    def test_unfinished_reasoning_and_ambiguous_refusal_not_valid(self):
        value=response();value['output'].insert(0,{'type':'reasoning','status':'in_progress','summary':[]})
        self.assertFalse(parse(value)['protocol_valid'])
        value=response();value['output'][0]['content'][0]['refusal']='declined'
        self.assertFalse(parse(value)['protocol_valid'])
    def test_false_text_never_from_usage_or_reasoning(self):
        value=response(output=[],output_text=TEXT)
        self.assertFalse(parse(value)['protocol_valid'])
        value=response(output=[{'type':'reasoning','summary':[{'text':TEXT}]}],output_text=TEXT)
        self.assertFalse(parse(value)['protocol_valid'])

    def test_unicode_and_escaped_duplicate_model_output(self):
        value=response(output_text='{"decision":"route","\\u0064ecision":"route"}',output=[{'type':'message','role':'assistant','content':[{'type':'output_text','text':'{"decision":"route","\\u0064ecision":"route"}'}]}])
        self.assertEqual(parse(value)['disposition'],'invalid_output')

if __name__=='__main__':unittest.main()
