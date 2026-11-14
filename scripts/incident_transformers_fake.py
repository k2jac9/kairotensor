"""Fixed offline fake dependencies for the actual new backend path; no ML imports."""
import contextlib
import copy
import time
from types import SimpleNamespace
from incident_transformers_backend import REQUIRED_VERSIONS

class FakeTensor:
    def __init__(self, values, dtype='torch.int64', device='cpu'):
        self.values=copy.deepcopy(values);self.dtype=dtype;self.device=SimpleNamespace(type=device)
    @property
    def shape(self): return (len(self.values),len(self.values[0]))
    def tolist(self):return copy.deepcopy(self.values)

class FakeConfig:
    def __init__(self):
        self.do_sample=True;self.num_beams=2;self.max_new_tokens=None;self.eos_token_id=99
    def to_dict(self):return copy.deepcopy(vars(self))

class FakeDependencies:
    def __init__(self, fault=None):
        fault={'wrong_prefix':'prefix','decode_failure':'decode'}.get(fault,fault)
        self.fault=fault;self.loads=[];self.tokenize_calls=0;self.render_calls=0;self.generate_calls=0
        self.versions=dict(REQUIRED_VERSIONS);self.fixture_identity='fixed-backend-fixture/1'
        if fault=='runtime_mismatch':self.versions['transformers']='invalid-version'
        self.asset_manifest={'config.json':'a'*64,'model.safetensors':'b'*64,'tokenizer.json':'c'*64}
        self.model_source='fixture-local-cache';self.last_inputs=None;self.received_inputs=None
        self.torch=SimpleNamespace(__version__=REQUIRED_VERSIONS['torch'], bfloat16='torch.bfloat16',
           get_num_threads=lambda:1, set_num_threads=lambda count:None,manual_seed=lambda value:None,
           inference_mode=contextlib.nullcontext)
        dep=self
        class Tokenizer:
            chat_template='fixture template'
            is_fast=True
            model_max_length=2048
            padding_side='right'
            truncation_side='right'
            special_tokens_map={'eos_token':'<eos>'}
            init_kwargs={'clean_up_tokenization_spaces':False,'add_bos_token':False,'name_or_path':'private-cache','token':'secret'}
            def apply_chat_template(self,messages,**kwargs):
                dep.render_calls+=1;dep.render_kwargs=kwargs
                return 'rendered:'+repr(messages)
            def __call__(self,text,**kwargs):
                dep.tokenize_calls+=1;dep.tokenize_kwargs=kwargs
                count=211 if text.count("'role':") == 4 else 173
                ids=[list(range(1,count+1))];mask=[[1]*count]
                if dep.fault=='bool_id':ids=[[True,2,3]]
                if dep.fault=='float_id':ids=[[1.0,2,3]]
                if dep.fault=='batch':ids=[[1,2,3],[1,2,3]]
                if dep.fault=='mask':mask[0][1]=0
                if dep.fault=='bool_mask':mask[0][1]=True
                if dep.fault=='too_long':ids=[list(range(2049))];mask=[[1]*2049]
                dep.last_inputs={'input_ids':FakeTensor(ids),'attention_mask':FakeTensor(mask)}
                return dep.last_inputs
            def decode(self,ids,**kwargs):
                dep.decoded_ids=copy.deepcopy(ids)
                if dep.fault=='decode':raise ValueError('decode failed')
                return '{"fixture":true}'
        class Model:
            def __init__(self):
                self.generation_config=FakeConfig();self.config=SimpleNamespace(to_dict=lambda:{'model_type':'qwen2'})
                if dep.fault=='eos_config':self.generation_config.eos_token_id=True
            def eval(self):return self
            def parameters(self):return iter([SimpleNamespace(dtype='torch.float32' if dep.fault=='dtype' else 'torch.bfloat16',device=SimpleNamespace(type='cuda' if dep.fault=='device' else 'cpu'))])
            def generate(self,**kwargs):
                dep.generate_calls+=1;dep.received_inputs=kwargs;dep.received_config=kwargs['generation_config']
                if dep.fault=='raise':raise RuntimeError('generation failed')
                if dep.fault=='hang_generation':time.sleep(3600)
                values=kwargs['input_ids'].tolist()[0]
                if dep.fault=='prefix':values=[9]+values[1:]
                completion=[4]*6+[99]
                if dep.fault=='cap_eos':completion=[4]*95+[99]
                if dep.fault=='overcap':completion=[4]*97
                if dep.fault=='output_batch':return FakeTensor([values+completion,values+completion])
                return FakeTensor([values+completion])
        def load_tokenizer(source,**kwargs):
            if dep.fault=='load_failure':raise RuntimeError('injected load failure')
            dep.loads.append(('tokenizer',source,kwargs));return Tokenizer()
        def load_model(source,**kwargs):dep.loads.append(('model',source,kwargs));return Model()
        self.transformers=SimpleNamespace(__version__=REQUIRED_VERSIONS['transformers'],
          AutoTokenizer=SimpleNamespace(from_pretrained=load_tokenizer),AutoModelForCausalLM=SimpleNamespace(from_pretrained=load_model))


def factory(fault='none'):
    return FakeDependencies(fault)
