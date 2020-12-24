#![feature(async_closure)]
mod store;
pub use store::Storage;
#[doc(hidden)] pub use store::FileStore;
mod setup;
pub use setup::{Setup,SetupFields};
mod tree;
pub use tree::{Tree,Tree2};
mod bytes;
mod query;
pub use query::QueryStream;
mod unfold;

use async_std::{sync::{Arc,Mutex}};
use random_access_storage::RandomAccess;
use desert::{ToBytes,FromBytes,CountBytes};
use core::ops::{Add,Div};

pub type Error = Box<dyn std::error::Error+Sync+Send>;
pub type Location = (u64,u32);

pub trait Value: Clone+core::fmt::Debug+Send+Sync+'static
  +ToBytes+CountBytes+FromBytes {}
pub trait Scalar: Clone+PartialOrd+From<u8>+core::fmt::Debug
  +ToBytes+CountBytes+FromBytes
  +Value+Add<Output=Self>+Div<Output=Self> {}
impl Value for f32 {}
impl Value for f64 {}
impl Scalar for f32 {}
impl Scalar for f64 {}
impl Value for u8 {}
impl Value for u16 {}
impl Value for u32 {}
impl Value for u64 {}
impl<T> Value for Vec<T> where T: Value {}

#[derive(Debug,Clone)]
pub enum Coord<X> where X: Scalar {
  Scalar(X),
  Interval(X,X)
}

#[async_trait::async_trait]
pub trait Point: 'static {
  type Bounds: Clone+Send+Sync+core::fmt::Debug+ToBytes+FromBytes+CountBytes;
  async fn batch<S,T,V>(db: &mut DB<S,T,Self,V>, rows: &[Row<Self,V>]) -> Result<(),Error>
    where S: RandomAccess<Error=Error>+Unpin+Send+Sync, V: Value, Self: Sized, T: Tree<Self,V>;
}

#[async_trait::async_trait]
impl<X,Y> Point for (Coord<X>,Coord<Y>) where X: Scalar, Y: Scalar {
  type Bounds = ((X,Y),(X,Y));
  async fn batch<S,T,V>(db: &mut DB<S,T,Self,V>, rows: &[Row<Self,V>]) -> Result<(),Error>
  where S: RandomAccess<Error=Error>+Unpin+Send+Sync, V: Value, T: Tree<(Coord<X>,Coord<Y>),V> {
    let inserts: Vec<(&Self,&V)> = rows.iter()
      .map(|row| match row {
        Row::Insert(p,v) => Some((p,v)),
        _ => None
      })
      .filter(|row| !row.is_none())
      .map(|x| x.unwrap())
      .collect();

    let merge_trees = db.trees.iter()
      .take_while(|t| { t.is_some() })
      .map(|t| { Arc::clone(&t.as_ref().unwrap()) })
      .collect::<Vec<Arc<Mutex<T>>>>();
    db.trees.insert(merge_trees.len(), Some(Arc::new(Mutex::new(
      tree::merge(9, inserts.as_slice(), merge_trees.as_slice()).await
    ))));
    Ok(())
  }
}

pub enum Row<P,V> where P: Point, V: Value {
  Insert(P,V),
  Delete(Location)
}

pub struct DB<S,T,P,V> where S: RandomAccess<Error=Error>+Unpin+Send+Sync,
P: Point, V: Value, T: Tree<P,V> {
  pub storage: Box<dyn Storage<S>+Unpin+Send+Sync>,
  pub fields: SetupFields,
  pub trees: Vec<Option<Arc<Mutex<T>>>>,
  _point: std::marker::PhantomData<P>,
  _value: std::marker::PhantomData<V>,
}

impl<S,T,P,V> DB<S,T,P,V> where S: RandomAccess<Error=Error>+Unpin+Send+Sync,
P: Point, V: Value, T: Tree<P,V> {
  pub async fn open_from_setup(setup: Setup<S>) -> Result<Self,Error> {
    Ok(Self {
      storage: setup.storage.into(),
      fields: setup.fields,
      trees: vec![],
      _point: std::marker::PhantomData,
      _value: std::marker::PhantomData,
    })
  }
  pub async fn batch(&mut self, rows: &[Row<P,V>]) -> Result<(),Error> {
    P::batch(self, rows).await
  }
  pub async fn query(&mut self, bbox: &P::Bounds) -> Result<query::QStream<P,V>,Error> {
    let mut queries = vec![];
    for tree in self.trees.iter() {
      if let Some(t) = tree {
        queries.push(t.lock().await.query(bbox));
      }
    }
    <QueryStream<P,V>>::from_queries(queries)
  }
}
