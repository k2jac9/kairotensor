import unittest
import http.client
from incident_adaption_transport import _exchange, HOST, PATH, MAX_BODY

class Socket:
    def __init__(self):self.timeouts=[]
    def settimeout(self,value):self.timeouts.append(value)
class Response:
    def __init__(self,chunks,status=200,headers=()):self.chunks=list(chunks);self.status=status;self.headers=headers
    def getheaders(self):return self.headers
    def read1(self,size):
        if not self.chunks:return b''
        item=self.chunks.pop(0)
        if isinstance(item,Exception):raise item
        return item
class Connection:
    def __init__(self,response):self.response=response;self.sock=Socket();self.requests=[];self.closed=False
    def request(self,*args,**kwargs):self.requests.append((args,kwargs))
    def getresponse(self):return self.response
    def close(self):self.closed=True
class TransportTests(unittest.TestCase):
    def exchange(self,response,clock=lambda:1):
        connection=Connection(response);records=[];factories=[]
        def factory(*args,**kwargs):factories.append((args,kwargs));return connection
        result=_exchange(b'{"frozen":true}','TEST_ONLY_NOT_A_SECRET',deadline=5,clock=clock,
                         persist=lambda k,v:records.append((k,v)),connection_factory=factory)
        return result,records,connection,factories
    def test_fixed_origin_tls_single_post_and_safe_headers(self):
        r,events,c,f=self.exchange(Response([b'hello'],headers=[('Authorization','secret'),('X-Request-Id','r')]))
        self.assertTrue(r.complete);self.assertEqual(r.body,b'hello');self.assertEqual(len(c.requests),1)
        self.assertEqual(f[0][0],(HOST,));self.assertTrue(f[0][1]['context'].check_hostname)
        self.assertEqual(c.requests[0][0],('POST',PATH));self.assertTrue(c.closed)
        self.assertEqual(events[0][1]['headers'],[['x-request-id','r']])
    def test_redirect_and_http_errors_never_retry(self):
        for code in (301,307,429,500):
            with self.subTest(code=code):
                r,e,c,f=self.exchange(Response([b'error'],status=code,headers=[('Location','https://evil.test')]))
                self.assertEqual(r.status,code);self.assertEqual(len(c.requests),1);self.assertEqual(len(f),1)
    def test_partial_body_survives_error_no_exception_message(self):
        r,e,c,_=self.exchange(Response([b'partial',TimeoutError('secret appears here')]))
        self.assertFalse(r.complete);self.assertEqual(r.body,b'partial');self.assertEqual(r.error,'TimeoutError')
        self.assertNotIn('secret appears here',repr(e));self.assertTrue(c.closed)
    def test_incomplete_read_exception_preserves_partial_bytes(self):
        r,e,c,_=self.exchange(Response([b'first',http.client.IncompleteRead(b'partial',99)]))
        self.assertFalse(r.complete);self.assertEqual(r.body,b'firstpartial')
        self.assertEqual(r.error,'IncompleteRead')
    def test_size_cap_preserves_bounded_prefix(self):
        r,e,c,_=self.exchange(Response([b'a'*(MAX_BODY+1)]))
        self.assertFalse(r.complete);self.assertEqual(len(r.body),MAX_BODY)
    def test_exact_body_ceiling_is_complete(self):
        r,e,c,_=self.exchange(Response([b'a'*MAX_BODY],headers=[('Content-Length',str(MAX_BODY))]))
        self.assertTrue(r.complete);self.assertEqual(len(r.body),MAX_BODY)
    def test_expired_clock_never_connects(self):
        r,e,c,f=self.exchange(Response([]),clock=lambda:5)
        self.assertFalse(r.complete);self.assertEqual(f,[]);self.assertEqual(c.requests,[])
    def test_chunk_persistence_failure_stops_reads(self):
        c=Connection(Response([b'one',b'two']));calls=[]
        def persist(k,v):
            calls.append(k)
            if k=='chunk':raise OSError('secret path')
        r=_exchange(b'{}','test',deadline=5,clock=lambda:1,persist=persist,connection_factory=lambda *a,**k:c)
        self.assertFalse(r.complete);self.assertEqual(c.response.chunks,[b'two'])
        self.assertNotIn('secret path',repr(r))
    def test_short_content_length_is_partial_not_complete(self):
        r,e,c,_=self.exchange(Response([b'abc'],headers=[('Content-Length','9')]))
        self.assertFalse(r.complete);self.assertEqual(r.body,b'abc')
        self.assertFalse(any(k=='complete' for k,v in e))
    def test_invalid_credential_refused_before_factory(self):
        for key in ('','abc\r\nInjected:yes','test space','é'):
            with self.subTest(key=key),self.assertRaises(ValueError):
                _exchange(b'{}',key,deadline=5,clock=lambda:1,persist=lambda *a:None,
                          connection_factory=lambda *a,**k:self.fail('network factory'))

if __name__=='__main__':unittest.main()
