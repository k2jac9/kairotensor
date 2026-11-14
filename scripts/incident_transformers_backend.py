"""Pinned CPU backend. Importing this module never imports or loads ML libraries.

InvocationApproval is a trusted caller attestation, not an unforgeable capability.
Do not construct it from artifacts. Real invocation needs independent user approval.
"""
import copy
import hashlib
import importlib
import importlib.metadata
import os
from pathlib import Path
import platform
import time
from dataclasses import dataclass
from types import SimpleNamespace

from handoff_model import RequestHandler
from incident_preflight import MODEL, REVISION, CAP, MAX_INPUT, validate_envelope
from incident_routing import canonical, digest, require, _loads

WIRE_VERSION = 'incident-transformers/1'
REQUIRED_VERSIONS = {'torch':'2.6.0+cpu','transformers':'4.48.3','tokenizers':'0.21.0',
                     'huggingface_hub':'0.34.4','safetensors':'0.5.3','accelerate':'1.3.0',
                     'jinja2':'3.1.6','numpy':'1.26.4'}

@dataclass(frozen=True)
class InvocationApproval:
    kind: str = 'deny'


def _sha_file(path):
    value=hashlib.sha256()
    with path.open('rb') as handle:
        for part in iter(lambda:handle.read(1024*1024),b''): value.update(part)
    return value.hexdigest()


def _local_dependencies():
    # Called only after explicit local_cached_model invocation attestation.
    versions={name:importlib.metadata.version(name) for name in REQUIRED_VERSIONS}
    require(versions == REQUIRED_VERSIONS, 'pinned dependency versions required')
    torch=importlib.import_module('torch');transformers=importlib.import_module('transformers')
    hub=importlib.import_module('huggingface_hub')
    root=Path(hub.snapshot_download(MODEL,revision=REVISION,local_files_only=True,token=False))
    require(root.name == REVISION, 'cache snapshot is not pinned revision')
    assets={str(path.relative_to(root)):_sha_file(path) for path in sorted(root.rglob('*')) if path.is_file()}
    return SimpleNamespace(torch=torch,transformers=transformers,versions=versions,
                           model_source=str(root),asset_manifest=assets,fixture_identity=None)


def _json(value):
    # Round trip rejects unsupported values and non-finite JSON constants.
    return _loads(canonical(value))


def _parse_cpuinfo(text):
    require(type(text) is str and bool(text.strip()),'CPU observation unavailable')
    processors=[]
    for block in text.strip().split('\n\n'):
        fields={key.strip().lower():value.strip() for line in block.splitlines() if ':' in line for key,value in [line.split(':',1)]}
        if not fields:continue
        model=fields.get('model name') or fields.get('hardware') or fields.get('processor')
        flags=(fields.get('flags') or fields.get('features') or '').split()
        require(type(model) is str and model and not model.isdecimal() and flags,'CPU model/flags unavailable')
        processors.append((model,set(flags)))
    require(processors,'CPU observation unavailable')
    common=set.intersection(*(flags for _,flags in processors))
    require(common,'common CPU flags unavailable')
    return {'source':'linux_proc_cpuinfo','model_names':sorted({model for model,_ in processors}),
            'flags':sorted(common),'logical_processor_count':len(processors)}


def _cpu_identity(fake):
    if fake:return {'source':'synthetic','model_names':['synthetic-cpu'],'flags':['synthetic'],'logical_processor_count':1}
    return _parse_cpuinfo(Path('/proc/cpuinfo').read_text(encoding='utf-8'))


_TOKENIZER_INIT_KEYS=frozenset(('model_max_length','padding_side','truncation_side','clean_up_tokenization_spaces',
    'split_special_tokens','add_prefix_space','add_bos_token','add_eos_token','legacy','use_default_system_prompt',
    'spaces_between_special_tokens','errors','bos_token','eos_token','unk_token','pad_token','sep_token',
    'cls_token','mask_token','additional_special_tokens'))


def _token_setting(value):
    if value is None or type(value) in (str,bool,int,float):return _json(value)
    if type(value) in (list,tuple):return [_token_setting(item) for item in value]
    # Avoid importing tokenizers, while preserving AddedToken behavior exactly.
    require(type(value).__name__=='AddedToken','unsupported tokenizer setting object')
    fields={name:getattr(value,name) for name in ('content','single_word','lstrip','rstrip','normalized','special')}
    require(type(fields['content']) is str and all(type(fields[k]) is bool for k in fields if k!='content'),'invalid AddedToken fields')
    return {'added_token':fields}


def _tokenizer_config(tokenizer):
    init=tokenizer.init_kwargs;special=tokenizer.special_tokens_map
    require(type(init) is dict and all(type(k) is str for k in init),'tokenizer initialization metadata unavailable')
    require(type(special) is dict and all(type(k) is str for k in special),'special token metadata unavailable')
    require(type(tokenizer.is_fast) is bool and type(tokenizer.model_max_length) is int and tokenizer.model_max_length>0,'tokenizer shape metadata invalid')
    require(tokenizer.padding_side in ('left','right') and tokenizer.truncation_side in ('left','right'),'tokenizer side metadata invalid')
    return {'class_name':type(tokenizer).__name__,'module':type(tokenizer).__module__,'is_fast':tokenizer.is_fast,
            'model_max_length':tokenizer.model_max_length,'padding_side':tokenizer.padding_side,
            'truncation_side':tokenizer.truncation_side,'special_tokens_map':{k:_token_setting(v) for k,v in special.items()},
            'init_kwargs':{k:_token_setting(v) for k,v in init.items() if k in _TOKENIZER_INIT_KEYS},
            'omitted_init_keys':sorted(set(init)-_TOKENIZER_INIT_KEYS)}


def _matrix(tensor, *, width=None, mask=False):
    require(getattr(getattr(tensor,'device',None),'type',None) == 'cpu','CPU tensor required')
    require(str(getattr(tensor,'dtype',None)) in ('torch.int64','torch.int32'), 'integer tensor dtype required')
    value=tensor.tolist()
    require(type(value) is list and len(value)==1 and type(value[0]) is list,'one tensor batch required')
    require(all(type(x) is int and 0<=x<2**63 for x in value[0]),'integer token values required')
    require(tuple(tensor.shape)==(1,len(value[0])),'tensor shape mismatch')
    if width is not None: require(len(value[0])==width,'tensor width mismatch')
    if mask: require(all(x==1 for x in value[0]),'unmasked unpadded input required')
    return value


def _eos_ids(config):
    value=config.eos_token_id
    if value is None:return []
    value=[value] if type(value) is int else value
    require(type(value) in (list,tuple) and all(type(x) is int and 0<=x<2**63 for x in value),'EOS config invalid')
    return list(value)


class Backend:
    """Single-owner sequential backend; trusted dependencies, no transport retries."""
    def __init__(self, approval=None, dependency_factory=None):
        if type(approval) is not InvocationApproval or approval.kind not in ('offline_fake_dependencies','local_cached_model'):
            raise PermissionError('backend invocation denied before dependencies')
        if approval.kind == 'offline_fake_dependencies' and dependency_factory is None:
            raise PermissionError('offline invocation requires explicit fake dependencies')
        if approval.kind == 'local_cached_model' and dependency_factory is not None:
            raise PermissionError('real invocation uses only pinned local dependency loader')
        started=time.perf_counter_ns()
        deps=(dependency_factory or _local_dependencies)()
        require(deps.versions == REQUIRED_VERSIONS,'pinned dependency versions required')
        fake=approval.kind == 'offline_fake_dependencies'
        require((type(deps.fixture_identity) is str and bool(deps.fixture_identity)) if fake else deps.fixture_identity is None,'dependency provenance mismatch')
        require(type(deps.asset_manifest) is dict and deps.asset_manifest,'asset manifest required')
        require(all(type(k) is str and k and type(v) is str and len(v)==64 and all(c in '0123456789abcdef' for c in v) for k,v in deps.asset_manifest.items()),'asset hashes invalid')
        self.torch=deps.torch;self.transformers=deps.transformers
        require(str(self.torch.__version__)==REQUIRED_VERSIONS['torch'] and str(self.transformers.__version__)==REQUIRED_VERSIONS['transformers'],'loaded dependency version mismatch')
        threads=max(1,min(4,len(os.sched_getaffinity(0)) if hasattr(os,'sched_getaffinity') else (os.cpu_count() or 1)))
        self.torch.set_num_threads(threads);self.torch.manual_seed(0)
        source=dict(revision=REVISION,local_files_only=True,trust_remote_code=False,token=False)
        self.tokenizer=self.transformers.AutoTokenizer.from_pretrained(deps.model_source,**source)
        self.model=self.transformers.AutoModelForCausalLM.from_pretrained(deps.model_source,
             torch_dtype=self.torch.bfloat16,low_cpu_mem_usage=True,use_safetensors=True,**source).eval()
        _eos_ids(self.model.generation_config)
        parameters=list(self.model.parameters())
        require(parameters and all(str(p.dtype)=='torch.bfloat16' and p.device.type=='cpu' for p in parameters),'observed model must be CPU BF16')
        self._runtime={'backend':WIRE_VERSION,'mode':'offline_fake_dependencies' if fake else 'local_cached_model',
          'fixture_identity':deps.fixture_identity,'model':MODEL,'revision':REVISION,'dtype':'bfloat16','device':'cpu',
          'versions':dict(deps.versions),'asset_manifest':dict(deps.asset_manifest),
          'threads':self.torch.get_num_threads(),'affinity':sorted(os.sched_getaffinity(0)) if hasattr(os,'sched_getaffinity') else None,
          'machine_architecture':platform.machine(),'python_version':platform.python_version(),
          'cpu_identity':_cpu_identity(fake),'tokenizer_config':_tokenizer_config(self.tokenizer),
          'chat_template_sha256':digest(self.tokenizer.chat_template),
          'model_config':_json(self.model.config.to_dict()),'generation_config':_json(self.model.generation_config.to_dict()),
          'load_us':(time.perf_counter_ns()-started)//1000,
          'source_sha256':{name:_sha_file(Path(__file__).with_name(name)) for name in ('incident_transformers_backend.py','handoff_model.py','incident_preflight.py')},
          'tokenization_route':'render_once_then_tokenize_no_added_special_tokens/1'}
        require(type(self._runtime['threads']) is int and self._runtime['threads']>0,'thread observation invalid')
        self._runtime_sha=digest(self._runtime);self._prepared=None;self._observation=None
        self.handler=RequestHandler(self._tokenize,self._generate)

    def _envelope(self,op,reply=None,observation=None,error=None):
        return copy.deepcopy({'wire_version':WIRE_VERSION,'op':op,'runtime_sha256':self._runtime_sha,
                              'reply':reply,'observation':observation,'error':error})

    def ready(self): return self._envelope('ready',observation=self._runtime)

    def _tokenize(self,request):
        validate_envelope(canonical(request))
        rendered=self.tokenizer.apply_chat_template(request['messages'],tokenize=False,add_generation_prompt=True)
        require(type(rendered) is str and rendered,'rendered chat required');rendered.encode('utf-8')
        inputs=self.tokenizer(rendered,add_special_tokens=False,padding=False,truncation=False,return_tensors='pt',return_attention_mask=True)
        require(set(inputs)=={'input_ids','attention_mask'},'exact input tensors required')
        ids=_matrix(inputs['input_ids']);require(1<=len(ids[0])<=MAX_INPUT,'input envelope exceeded')
        mask=_matrix(inputs['attention_mask'],width=len(ids[0]),mask=True)
        config=copy.deepcopy(self.model.generation_config)
        for name,value in request['generation'].items(): setattr(config,name,value)
        config.num_return_sequences=1;config.return_dict_in_generate=False
        config.output_scores=False;config.output_logits=False
        _eos_ids(config)
        effective=_json(config.to_dict())
        observation={'request_sha256':self._request_sha,'handle':None,'rendered_chat':rendered,
          'input_ids':ids,'attention_mask':mask,'effective_generation_config':effective,'runtime_sha256':self._runtime_sha}
        self._prepared={'observation':observation,'inputs':inputs,'config':config}
        return inputs,{'max_new_tokens':CAP,'config':config}

    def _generate(self,inputs,generation):
        prepared=self._prepared;require(prepared is not None,'missing prepared observation')
        frozen=prepared['observation']
        require(inputs is prepared['inputs'] and generation['config'] is prepared['config'],'prepared objects changed')
        require(_matrix(inputs['input_ids'])==frozen['input_ids'] and _matrix(inputs['attention_mask'],width=len(frozen['input_ids'][0]),mask=True)==frozen['attention_mask'],'prepared tensors mutated')
        require(_json(generation['config'].to_dict())==frozen['effective_generation_config'],'prepared config mutated')
        start=time.perf_counter_ns()
        with self.torch.inference_mode(): result=self.model.generate(**inputs,generation_config=generation['config'])
        elapsed=(time.perf_counter_ns()-start)//1000
        output=_matrix(result)[0];length=len(frozen['input_ids'][0])
        self._observation={'request_sha256':frozen['request_sha256'],'handle':frozen['handle'],
          'prepared_sha256':frozen['prepared_sha256'],'raw_output_ids':[output],'usage_known':False}
        require(output[:length]==frozen['input_ids'][0] and len(output)>=length,'generated prefix mismatch')
        continuation=output[length:];require(len(continuation)<=CAP,'generated output cap exceeded')
        eos=_eos_ids(generation['config'])
        self._observation={'request_sha256':frozen['request_sha256'],'handle':frozen['handle'],
          'prepared_sha256':frozen['prepared_sha256'],'continuation_ids':continuation,
          'input_tokens':length,'output_tokens':len(continuation),'generation_us':elapsed,
          'usage_known':True,'hit_cap':len(continuation)==CAP,'ended_eos':bool(continuation) and continuation[-1] in eos}
        text=self.tokenizer.decode(continuation,skip_special_tokens=True)
        require(type(text) is str,'decoded text required');text.encode('utf-8')
        return {key:self._observation[key] for key in ('input_tokens','output_tokens','generation_us','hit_cap','ended_eos')} | {'text':text}

    def handle(self,command):
        op=command.get('op') if type(command) is dict else None
        self._observation=None
        try:
            require(op in ('prepare','generate','discard'),'only prepared operations are supported')
            if op=='prepare':
                request=command.get('request_utf8');validate_envelope(request)
                self._request_sha=hashlib.sha256(request.encode('utf-8')).hexdigest()
            reply=self.handler.handle(command)
            if op=='prepare':
                observation=self._prepared['observation'];observation['handle']=reply['handle']
                observation['prepared_sha256']=digest(observation)
                self._observation=observation
            elif op=='discard':
                self._observation={'handle':reply['discarded'],'request_sha256':self._prepared['observation']['request_sha256'],
                                   'prepared_sha256':self._prepared['observation']['prepared_sha256']}
                self._prepared=None
            return self._envelope(op,reply,self._observation)
        except Exception as error:
            return self._envelope(op,observation=self._observation,error=f'{type(error).__name__}: {error}')
