extern crate random_access_storage;
extern crate failure;
extern crate bincode;
extern crate serde;

mod meta;
mod point;
mod tree;
mod branch;
mod staging;
mod planner;
mod bits;

use staging::{Staging,StagingIterator};
use planner::plan;
pub use point::{Point,Scalar};
pub use tree::{Tree,TreeIterator};

use random_access_storage::RandomAccess;
use failure::Error;
use serde::{Serialize,de::DeserializeOwned};
use meta::Meta;
use std::marker::PhantomData;
use std::fmt::Debug;
use std::cell::RefCell;

enum SubIterator<'a,'b,S,P,V>
where S: RandomAccess<Error=Error>, P: Point, V: Value {
  Tree(TreeIterator<'a,'b,S,P,V>),
  Staging(StagingIterator<'a,'b,P,V>)
}

pub trait Value: Debug+Copy+Serialize+DeserializeOwned+'static {}
impl<T> Value for T where T: Debug+Copy+Serialize+DeserializeOwned+'static {}

#[derive(Clone,Debug)]
pub enum Row<P,V> where P: Point, V: Value {
  Insert(P,V),
  Delete(P,V)
}

pub struct DB<S,U,P,V> where
S: RandomAccess<Error=Error>,
U: (Fn(&str) -> Result<S,Error>),
P: Point, V: Value {
  open_store: U,
  trees: Vec<Tree<S,P,V>>,
  order: RefCell<Vec<usize>>,
  staging: Staging<S,P,V>,
  meta: Meta<S>,
  _marker: PhantomData<(P,V)>,
}

impl<S,U,P,V> DB<S,U,P,V> where
S: RandomAccess<Error=Error>,
U: (Fn(&str) -> Result<S,Error>),
P: Point, V: Value {
  pub fn open(open_store: U) -> Result<Self,Error> {
    let meta = Meta::open(open_store("meta")?)?;
    let staging = Staging::open(open_store("staging")?)?;
    let bf = 8;
    Ok(Self {
      open_store,
      staging,
      order: RefCell::new(pivot_order(bf)),
      meta: meta,
      trees: vec![],
      _marker: PhantomData
    })
  }
  pub fn batch (&mut self, rows: &Vec<Row<P,V>>) -> Result<(),Error> {
    let base = 8_u64.pow(2);
    let n = (self.staging.len()? + rows.len()) as u64;
    if n > base {
      let count = (n/base)*base;
      let rem = n - count;
      let mut mask = vec![];
      for mut tree in self.trees.iter_mut() {
        mask.push(tree.is_empty()?);
      }
      let p = plan(
        &bits::num_to_bits(n/base),
        &mask
      );
      let mut offset = 0;
      let mut last_staging = 0;
      let mut last_rows = 0;
      for (i,staging,trees) in p {
        let slen = self.staging.rows.len();
        let mut srows = vec![];
        for j in staging {
          let size = (2u64.pow(j as u32) * base) as usize;
          let start = offset;
          let end = start + size;
          offset = end;
          if end <= slen {
            srows.extend_from_slice(&self.staging.rows[start..end]);
            last_staging = last_staging.max(end);
          } else if start >= slen {
            srows.extend_from_slice(&rows[start-slen..end-slen]);
            last_rows = last_rows.max(end-slen);
          } else {
            srows.extend_from_slice(&self.staging.rows[start..slen]);
            srows.extend_from_slice(&rows[0..end-slen]);
            last_staging = last_staging.max(slen);
            last_rows = last_rows.max(slen);
          }
        }
        for t in trees.iter() {
          self.create_tree(*t)?;
        }
        self.create_tree(i)?;
        // trees[i].merge(...)
      }
      let mut rem_rows = vec![];
      if last_staging < self.staging.rows.len() && self.staging.rows.len() > 0 {
        rem_rows.extend_from_slice(&self.staging.rows[last_staging..]);
      }
      if last_rows < rows.len() && rows.len() > 0 {
        rem_rows.extend_from_slice(&rows[last_rows..]);
      }
      assert!(rem_rows.len() == rem as usize,
        "expected number of remaining rows");
      self.staging.clear()?;
      self.staging.batch(&rem_rows)?;
    } else {
      self.staging.batch(rows)?;
    }
    Ok(())
  }
  fn create_tree (&mut self, index: usize) -> Result<(),Error> {
    for i in self.trees.len()..index+1 {
      let store = (self.open_store)(&format!("tree{}",i))?;
      self.trees.push(Tree::open(store, 8, 100, self.order.clone())?);
    }
    Ok(())
  }
  pub fn query<'a,'b> (&'a mut self, bbox: &'b P::BBox)
  -> Result<QueryIterator<'a,'b,S,P,V>,Error> {
    QueryIterator::new(self, bbox)
  }
}

pub struct QueryIterator<'a,'b,S,P,V> where
S: RandomAccess<Error=Error>, P: Point, V: Value {
  index: usize,
  queries: Vec<SubIterator<'a,'b,S,P,V>>
}

impl<'a,'b,S,P,V> QueryIterator<'a,'b,S,P,V> where
S: RandomAccess<Error=Error>, P: Point, V: Value {
  pub fn new<U> (db: &'a mut DB<S,U,P,V>, bbox: &'b P::BBox)
  -> Result<Self,Error>
  where U: (Fn(&str) -> Result<S,Error>) {
    let mut mask: Vec<bool> = vec![];
    for tree in db.trees.iter_mut() {
      mask.push(tree.is_empty()?);
    }
    let mut queries: Vec<SubIterator<'a,'b,S,P,V>>
      = Vec::with_capacity(1+db.trees.len());
    queries.push(SubIterator::Staging(db.staging.query(bbox)));
    let exq: Vec<SubIterator<'a,'b,S,P,V>>
      = db.trees.iter_mut().enumerate()
        .filter(|(i,_tree)| { mask[*i] })
        .map(|(_i,tree)| { SubIterator::Tree(tree.query(bbox)) })
        .collect();
    queries.extend(exq);
    Ok(Self { queries, index: 0 })
  }
}

impl<'a,'b,S,P,V> Iterator for QueryIterator<'a,'b,S,P,V> where
S: RandomAccess<Error=Error>, P: Point, V: Value {
  type Item = Result<(P,V),Error>;
  fn next (&mut self) -> Option<Self::Item> {
    while !self.queries.is_empty() {
      let len = self.queries.len();
      {
        let q = &mut self.queries[self.index];
        let next = match q {
          SubIterator::Tree(x) => x.next(),
          SubIterator::Staging(x) => x.next()
        };
        match next {
          Some(result) => {
            self.index = (self.index+1) % len;
            return Some(result);
          },
          None => {}
        }
      }
      self.queries.remove(self.index);
      self.index = self.index % len;
    }
    None
  }
}

fn pivot_order (bf: usize) -> Vec<usize> {
  let n = bf*2-1;
  let mut order = Vec::with_capacity(n);
  for i in 0..((((n+1) as f32).log2()) as usize) {
    let m = 2usize.pow(i as u32);
    for j in 0..m {
      order.push(n/(m*2) + j*(n+1)/m);
    }
  }
  order
}
