//! Resumable control flow; await-free operations reuse the existing evaluator.
use super::{parser::{Expr,Stmt,Property},vm::{Engine,Value,FunctionKind,ScriptError}};
use crate::dual::Id;
use std::sync::Arc;
type Result<T> = std::result::Result<T,ScriptError>;
#[derive(Clone)]pub(super) enum Op {
 Eval(Expr,usize), Invoke{callee:usize,receiver:Option<usize>,args:Vec<usize>,new:bool,out:usize}, Key(usize),
 Init(String,usize,bool), Enter(Vec<Stmt>), Exit, Hoist(Vec<Stmt>), Branch(usize,usize,usize), Jump(usize),
 Await(usize,usize), Return(usize), Throw(usize), Object(usize), Array(usize,u32),
 Property(usize,String,Property), Item(usize,String,usize),
 Try{catch:Option<usize>,finally:Option<usize>,end:usize}, Catch(String,Vec<Stmt>), EndTry, EndFinally, Sync(Stmt),
}
pub(super) struct Program{ops:Vec<Op>,registers:usize}
fn name(r:usize)->String{format!("\0async{r}")}
fn read(r:usize)->Expr{Expr::Name(name(r))}
fn expr_await(e:&Expr)->bool{match e{
 Expr::Await(_)=>true,Expr::Function{..}|Expr::Arrow{..}|Expr::AsyncFunction{..}|Expr::AsyncArrow{..}=>false,
 Expr::Member(a,_)|Expr::Unary(_,a)=>expr_await(a),Expr::Computed(a,b)|Expr::Binary(_,a,b)|Expr::Assign(a,b)=>expr_await(a)||expr_await(b),
 Expr::Call(a,b)|Expr::New(a,b)=>expr_await(a)||b.iter().any(expr_await),Expr::Array(e)=>e.iter().flatten().any(expr_await),
 Expr::Object(props)=>props.iter().any(|(_,p)|matches!(p,Property::Value(e)|Property::Prototype(e)if expr_await(e))),_=>false}}
fn stmt_await(s:&Stmt)->bool{match s{
 Stmt::Const(_,e)|Stmt::Let(_,e)|Stmt::Return(e)|Stmt::Throw(e)|Stmt::Expr(e)=>expr_await(e),Stmt::Var(_,e)=>e.as_ref().is_some_and(expr_await),
 Stmt::Set(a,_,b)=>expr_await(a)||expr_await(b),Stmt::Block(b)=>b.iter().any(stmt_await),Stmt::If(e,a,b)=>expr_await(e)||stmt_await(a)||b.as_ref().is_some_and(|b|stmt_await(b)),
 Stmt::Try{body,catch,finally}=>body.iter().any(stmt_await)||catch.as_ref().is_some_and(|(_,b)|b.iter().any(stmt_await))||finally.as_ref().is_some_and(|b|b.iter().any(stmt_await)),
 Stmt::ForOf{iterable,body,..}=>expr_await(iterable)||stmt_await(body),_=>false}}
struct Compiler{ops:Vec<Op>,next:usize}
impl Compiler{
 fn emit(&mut self,op:Op)->usize{let at=self.ops.len();self.ops.push(op);at}
 fn reg(&mut self)->usize{let r=self.next;self.next+=1;r}
 fn eval(&mut self,e:Expr)->usize{let r=self.reg();self.emit(Op::Eval(e,r));r}
 fn reference(&mut self,e:&Expr)->Result<(usize,Expr)>{match e{
  Expr::Member(base,key)=>{let base=self.expression(base)?;Ok((base,Expr::Member(Box::new(read(base)),key.clone())))},
  Expr::Computed(base,key)=>{let base=self.expression(base)?;let key=self.expression(key)?;self.emit(Op::Key(key));Ok((base,Expr::Computed(Box::new(read(base)),Box::new(read(key)))))},
  _=>Err(ScriptError::Unsupported("async reference"))}}
 fn expression(&mut self,e:&Expr)->Result<usize>{
  if !expr_await(e){return Ok(self.eval(e.clone()));}
  Ok(match e{
   Expr::Await(e)=>{let value=self.expression(e)?;let out=self.reg();self.emit(Op::Await(value,out));out},
   Expr::Call(callee,args)|Expr::New(callee,args)=>{let new=matches!(e,Expr::New(..));let(receiver,callee)=if !new&&matches!(callee.as_ref(),Expr::Member(..)|Expr::Computed(..)){let(base,expr)=self.reference(callee)?;(Some(base),self.eval(expr))}else{(None,self.expression(callee)?)};let args=args.iter().map(|e|self.expression(e)).collect::<Result<Vec<_>>>()?;let out=self.reg();self.emit(Op::Invoke{callee,receiver,args,new,out});out},
   Expr::Member(..)|Expr::Computed(..)=>{let(_,reference)=self.reference(e)?;self.eval(reference)},
   Expr::Assign(target,value)=>{let target=if matches!(target.as_ref(),Expr::Name(_)){target.as_ref().clone()}else{self.reference(target)?.1};let value=self.expression(value)?;self.eval(Expr::Assign(Box::new(target),Box::new(read(value))))},
   Expr::Unary(op,e)if op=="delete"=>{let reference=if matches!(e.as_ref(),Expr::Member(..)|Expr::Computed(..)){self.reference(e)?.1}else{read(self.expression(e)?)};self.eval(Expr::Unary(op.clone(),Box::new(reference)))},
   Expr::Unary(op,e)=>{let value=self.expression(e)?;self.eval(Expr::Unary(op.clone(),Box::new(read(value))))},
   Expr::Binary(op,a,b)if op=="&&"||op=="||"=>{let left=self.expression(a)?;let out=self.reg();let branch=self.emit(Op::Branch(left,0,0));let right_start=self.ops.len();let right=self.expression(b)?;self.emit(Op::Eval(read(right),out));let jump=self.emit(Op::Jump(0));let short=self.ops.len();self.emit(Op::Eval(read(left),out));let end=self.ops.len();self.ops[jump]=Op::Jump(end);self.ops[branch]=if op=="&&"{Op::Branch(left,right_start,short)}else{Op::Branch(left,short,right_start)};out},
   Expr::Binary(op,a,b)=>{let a=self.expression(a)?;let b=self.expression(b)?;self.eval(Expr::Binary(op.clone(),Box::new(read(a)),Box::new(read(b))))},
   Expr::Array(elements)=>{let out=self.reg();self.emit(Op::Array(out,elements.len() as u32));for(i,e)in elements.iter().enumerate(){if let Some(e)=e{let value=self.expression(e)?;self.emit(Op::Item(out,i.to_string(),value));}}out},
   Expr::Object(props)=>{let out=self.reg();self.emit(Op::Object(out));for(key,p)in props{let p=match p{Property::Value(e)=>Property::Value(read(self.expression(e)?)),Property::Prototype(e)=>Property::Prototype(read(self.expression(e)?)),p=>p.clone()};self.emit(Op::Property(out,key.clone(),p));}out},
   _=>return Err(ScriptError::Unsupported("await expression shape"))})
 }
 fn body(&mut self,body:&[Stmt],scope:bool)->Result<()>{if scope{self.emit(Op::Enter(body.to_vec()));}else{self.emit(Op::Hoist(body.to_vec()));}for s in body{self.statement(s)?;}if scope{self.emit(Op::Exit);}Ok(())}
 fn statement(&mut self,s:&Stmt)->Result<()>{match s{
  Stmt::Function(..)|Stmt::AsyncFunction(..)=>{},
  Stmt::Const(n,e)|Stmt::Let(n,e)=>{let value=self.expression(e)?;self.emit(Op::Init(n.clone(),value,false));},Stmt::Var(n,e)=>if let Some(e)=e{let value=self.expression(e)?;self.emit(Op::Init(n.clone(),value,true));},
  Stmt::Expr(e)=>{self.expression(e)?;},Stmt::Return(e)=>{let r=self.expression(e)?;self.emit(Op::Return(r));},Stmt::Throw(e)=>{let r=self.expression(e)?;self.emit(Op::Throw(r));},
  Stmt::Set(a,k,b)=>{self.expression(&Expr::Assign(Box::new(Expr::Member(Box::new(a.clone()),k.clone())),Box::new(b.clone())))?;},
  Stmt::Block(b)=>self.body(b,true)?,Stmt::If(c,a,b)=>{let test=self.expression(c)?;let branch=self.emit(Op::Branch(test,0,0));let yes=self.ops.len();self.statement(a)?;let jump=self.emit(Op::Jump(0));let no=self.ops.len();if let Some(b)=b{self.statement(b)?;}let end=self.ops.len();self.ops[jump]=Op::Jump(end);self.ops[branch]=Op::Branch(test,yes,no);},
  Stmt::Try{body,catch,finally}=>{let start=self.emit(Op::Try{catch:None,finally:None,end:0});self.body(body,true)?;self.emit(Op::EndTry);let catch_ip=if let Some((name,body))=catch{let at=self.emit(Op::Catch(name.clone(),body.clone()));for s in body{self.statement(s)?;}self.emit(Op::EndTry);Some(at)}else{None};let finally_ip=if let Some(body)=finally{let at=self.ops.len();self.body(body,true)?;self.emit(Op::EndFinally);Some(at)}else{None};let end=self.ops.len();self.ops[start]=Op::Try{catch:catch_ip,finally:finally_ip,end};},
  Stmt::ForOf{..}if stmt_await(s)=>return Err(ScriptError::Unsupported("await inside for-of is not implemented")),s=>{self.emit(Op::Sync(s.clone()));}
 }Ok(())}
}
pub(super) fn compile(body:&[Stmt])->Result<Arc<Program>>{let mut c=Compiler{ops:vec![],next:0};c.body(body,false)?;if c.ops.len()>16384{return Err(ScriptError::Runtime("async instruction limit"));}Ok(Arc::new(Program{ops:c.ops,registers:c.next}))}
impl Engine<'_>{pub(super) fn start_async(&mut self,_env:Id,_params:Vec<String>,_body:Vec<Stmt>,_kind:FunctionKind,_receiver:Value,_args:Vec<Value>,_program:Arc<Program>)->Result<Value>{Err(ScriptError::Unsupported("async execution pending"))}}
