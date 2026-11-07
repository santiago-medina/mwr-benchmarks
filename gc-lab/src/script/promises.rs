//! Promise state and jobs are ordinary traced records; queue entries are explicit roots.
use super::{objects::Data,vm::{Engine,Value,ScriptError}};
use crate::dual::{Id,ExternalRoot};
use std::collections::VecDeque;
type Result<T> = std::result::Result<T,ScriptError>;
#[derive(Clone)]
pub(super) struct Capability {pub promise:Value,pub resolve:Value,pub reject:Value}
impl Capability {fn values(&self)->Vec<Value>{vec![self.promise.clone(),self.resolve.clone(),self.reject.clone()]}}
#[derive(Clone)]
pub(super) struct Reaction{pub fulfilled:Option<Value>,pub rejected:Option<Value>,pub capability:Capability}
#[derive(Clone)]
pub(super) enum State {Pending,Fulfilled(Value),Rejected(Value)}
#[derive(Clone)]
pub(super) struct PromiseState {pub state:State,pub reactions:Vec<Reaction>,pub handled:bool}
impl PromiseState {pub fn edges(&self)->Vec<Id>{let mut values=vec![];match &self.state{State::Fulfilled(v)|State::Rejected(v)=>values.push(v.clone()),State::Pending=>{}}for r in &self.reactions{values.extend(r.fulfilled.clone());values.extend(r.rejected.clone());values.extend(r.capability.values());}values.iter().filter_map(Value::id).collect()}}
#[derive(Clone)]
pub(super) enum Internal {Resolve{cell:Id,reject:bool},Capability{cell:Id},Finally{handler:Value,constructor:Value,reject:bool},Thunk{value:Value,reject:bool}}
impl Internal {pub fn edges(&self)->Vec<Id>{match self{Self::Resolve{cell,..}|Self::Capability{cell}=>vec![*cell],Self::Finally{handler,constructor,..}=>[handler,constructor].into_iter().filter_map(Value::id).collect(),Self::Thunk{value,..}=>value.id().into_iter().collect()}}}
#[derive(Clone)]
pub(super) enum Cell {Resolving{promise:Id,resolved:bool},Capability{resolve:Value,reject:Value}}
impl Cell {pub fn edges(&self)->Vec<Id>{match self{Self::Resolving{promise,..}=>vec![*promise],Self::Capability{resolve,reject}=>[resolve,reject].into_iter().filter_map(Value::id).collect()}}}
#[derive(Clone)]
pub(super) enum Job {Reaction{handler:Option<Value>,argument:Value,reject:bool,capability:Capability},Thenable{thenable:Value,then:Value,resolve:Value,reject:Value},Callback(Value)}
impl Job {pub fn edges(&self)->Vec<Id>{let values=match self{Self::Reaction{handler,argument,capability,..}=>{let mut values=vec![argument.clone()];values.extend(handler.clone());values.extend(capability.values());values},Self::Thenable{thenable,then,resolve,reject}=>vec![thenable.clone(),then.clone(),resolve.clone(),reject.clone()],Self::Callback(f)=>vec![f.clone()]};values.iter().filter_map(Value::id).collect()}}
pub(super) struct QueuedJob {pub serial:u64,pub root:ExternalRoot}
impl Engine<'_>{
 pub fn pending_microtasks(&self)->usize{self.jobs.len()}
 pub fn cancel_microtasks(&mut self){let jobs=std::mem::take(&mut self.jobs);for job in jobs{self.runtime.unroot(job.root).expect("owned microtask root");}}
 pub fn microtask_checkpoint(&mut self,limit:usize)->Result<usize>{
  if self.call_depth!=0||self.native_depth!=0||self.checkpoint_running{return Err(ScriptError::Runtime("reentrant microtask checkpoint"));}
  self.checkpoint_running=true;let result=(||{let mut count=0;while !self.jobs.is_empty(){if count>=limit{return Err(ScriptError::Runtime("microtask checkpoint limit"));}self.run_one_microtask(None)?;count+=1;}Ok(count)})();self.checkpoint_running=false;
  if result.is_err(){self.cancel_microtasks();}self.collect();result
 }
 /// Run exactly the host-scheduled job. Stale/cancelled tokens are harmless.
 pub fn run_one_microtask(&mut self,serial:Option<u64>)->Result<bool>{
  if self.call_depth!=0||self.native_depth!=0{return Err(ScriptError::Runtime("microtask execution during script"));}
  let Some(front)=self.jobs.front()else{return Ok(false)};if serial.is_some_and(|s|s!=front.serial){return Ok(false);}
  let root=front.root;let id=self.runtime.resolve(root)?;self.steps_for_external_event();
  let result=self.frame(|e|{e.protect(&Value::Object(id))?;let queued=e.jobs.pop_front().unwrap();e.runtime.unroot(queued.root)?;let job=e.data(id)?.job.clone().ok_or(ScriptError::Runtime("missing job record"))?;e.run_job(job)});
  if result.is_err(){self.cancel_microtasks();}self.collect();result.map(|_|true)
 }
 pub(super) fn enqueue_job(&mut self,job:Job)->Result<()>{
  let id=self.new_object(None)?;let mut data=Data::new(None);data.job=Some(job);self.replace_data(id,data)?;
  let root=self.runtime.root(id)?;let serial=self.next_job;self.next_job=self.next_job.checked_add(1).ok_or(ScriptError::Runtime("job serial limit"))?;self.jobs.push_back(QueuedJob{serial,root});
  if let Some(adapter)=self.adapter.as_mut(){if let Err(error)=adapter.queue_microtask(serial){let queued=self.jobs.pop_back().unwrap();self.runtime.unroot(queued.root)?;self.adapter_error=Some(error);return Err(ScriptError::Runtime("host microtask scheduling failed"));}}
  Ok(())
 }
 pub(super) fn run_job(&mut self,_job:Job)->Result<()>{Err(ScriptError::Unsupported("promise job execution pending"))}
}
pub(super) fn empty_queue()->VecDeque<QueuedJob>{VecDeque::new()}
