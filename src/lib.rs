#![deny(warnings)]
#![feature(plugin)]
//#![plugin(clippy)]
//#![warn(missing_docs)]

#[macro_use]
extern crate lua;
extern crate libc;

pub use self::libc::{c_int,c_void};
use lua::{ThreadStatus, Index};
use std::rc::Rc;
use std::cell::{RefCell};
use std::cell;
use std::ptr;
use std::marker::PhantomData;
use std::clone::Clone;
use std::collections::hash_map::HashMap;
use std::any::{Any, TypeId};
use std::error::Error;
use std::fmt::{Display,Formatter};

/* Smart wrapper for types shared with Lua */
pub struct LuaPtr<T> {
    obj: Rc<RefCell<T>>,
}

impl<T> Clone for LuaPtr<T> {
    fn clone(&self) -> Self {
        LuaPtr{obj: self.obj.clone()}
    }
}

impl<T> LuaPtr<T> {
    pub fn new(obj: T) -> LuaPtr<T> {
        LuaPtr{
            obj: Rc::new(RefCell::new(obj)),
        }
    }
    pub fn borrow_mut<'a>(&'a mut self) -> cell::RefMut<'a, T> where T:'a {
        (*self.obj).borrow_mut()
    }
    pub fn borrow(&self) -> cell::Ref<T> {
        (*self.obj).borrow()
    }
}

/// Lua function which helps translate from Rust's Result<>
/// to Lua-style error.
const LUA_FUNC_SHIM: &'static str = r#"
    local rust_f, fname = ...
    local function check(ok, ...)
        if ok then
            return ...
        else
            local msg = ...
            error("Calling "..tostring(fname)..":\n"..msg, 2)
        end
    end
    function f(...)
        return check(rust_f(...))
    end
    return f
"#;

/* Lua interface */
pub struct RumLua<'a> {
    pub state: lua::State,
    types_str_to_id: HashMap<String, TypeId>,
    types_id_to_str: HashMap<TypeId, String>,
    lua_func_shim: lua::Reference,
    marker: PhantomData<&'a ()>,
}

pub type LuaError = Box<Error>;

#[derive(Debug)]
pub struct LError {
    message: String,
}

impl Error for LError {
    fn description(&self) -> &str {
        &self.message
    }
    fn cause(&self) -> Option<&Error> { None }
}

impl Display for LError {
    fn fmt(&self, f: &mut Formatter) -> Result<(), std::fmt::Error> {
        write!(f, "Error: {}", self.message).unwrap();
        Ok(())
    }
}

pub type LuaRet = Result<isize, LuaError>;
pub type Callback = fn(&mut RumLua) -> LuaRet;

// Return a LuaRet with an error string.
pub fn lfail<T>(message: &str) -> Result<T, LuaError> {
    Err(lerror(message))
}
// Return a LuaError (not wrapped in Result<>)
pub fn lerror(message: &str) -> LuaError {
    Box::new(LError{message: message.to_string()})
}

pub struct LuaType {
    pub methods: &'static [(&'static str, Callback)],
}

impl<'a> RumLua<'a> {
//    #[allow(new_without_default)]
    pub fn new() -> RumLua<'a> {
        let mut state = lua::State::new();
        state.open_libs();

        state.load_string(LUA_FUNC_SHIM);
        let lua_func_shim = state.reference(lua::REGISTRYINDEX);
        let mut result = RumLua{
            state: state,
            types_id_to_str: HashMap::new(),
            types_str_to_id: HashMap::new(),
            lua_func_shim: lua_func_shim,
            marker: PhantomData,
        };
        result.add_rum_libs();
        result
    }

    /* Run the Lua function at the top of the stack, with the
     * error catching.
     */
    pub fn run_loaded_lua(&mut self, num_args: i32, num_results: i32)
                          -> Result<(), LuaError> {
        self.state.get_global("debug");
        self.state.get_field(-1, "traceback");
        self.state.remove(-2);
        let msgh_pos = self.state.get_top() - 1 - num_args;
        // Swap with chunk to execute
        self.state.rotate(-2-num_args, 1);
        let status = self.state.pcall(num_args, num_results, msgh_pos);
        // Remove message handler
        match status {
            ThreadStatus::Ok => {
                self.state.remove(msgh_pos);
                Ok(())
            },
            _ => {
                self.state.remove(-3); // message handler below err,msg
                let err_msg = self.state.to_str(-1);
                match err_msg {
                    Some(msg) => lfail(&format!("Error running Lua: {}", msg)),
                    _ => lfail("Error loading string"),
                }
            },
        }
    }

    pub fn dump_stack(&mut self, message: &str) {
        let top = self.state.get_top();
        println!("Lua stack dump ({} items); {}", top, message);
        for i in 1..top+1 {
            match self.state.to_str(i) {
                Some(s) => println!("  {}: {}", i, s),
                _ => println!("  {}: ???", i),
            }
            self.state.pop(1);
        }
    }

    #[allow(dead_code)]
    pub fn do_string(&mut self, s: &str) {
        let status = self.state.load_string(s);
        match status {
            ThreadStatus::Ok => {
                    self.run_loaded_lua(0, 0).unwrap()
                },
            _ => {
                let err_msg = self.state.to_str(-1);
                match err_msg {
                    Some(msg) => println!("Syntax error loading string: {}", msg),
                    _ => println!("Error loading string"),
                }
            }
        }
        /* Clear the stack... */
        // TODO: only pop what we put there
        let size = self.state.get_top();
        self.state.pop(size);
    }

    pub fn do_file(&mut self, path: &str) -> Result<(),LuaError> {
        let status = self.state.load_file(path);
        match status {
            ThreadStatus::Ok => {
                    try!(self.run_loaded_lua(0, 0))
                },
            _ => {
                let err_msg = self.state.to_str(-1);
                return match err_msg {
                    Some(err_msg) => lfail(&format!("Syntax error loading file: {}", err_msg)),
                    _ => lfail("Error loading file"),
                }
            }
        }
        /* Clear the stack... */
        // TODO: only pop what we put there
        let size = self.state.get_top();
        self.state.pop(size);
        Ok(())
    }

    fn add_rum_libs(&mut self) {
        self.state.new_table();
        self.state.set_global("rum");
    }

    fn lua_func_wrapper(state: &mut lua::State) -> c_int {
        let rl_obj: &mut RumLua = unsafe {
            let rl_ptr = state.to_userdata(lua::ffi::lua_upvalueindex(1));
            &mut *(rl_ptr as *mut RumLua)
        };
        let f: &mut Box<Callback> = unsafe {
            let f_ptr: *mut Box<Callback> = state.to_userdata(lua::ffi::lua_upvalueindex(2)) as *mut Box<Callback>;
            &mut *f_ptr
        };
        match f(rl_obj) {
            Ok(num_results) => {
                /* The results are on the top of the stask.  We need to
                 * push a "true" underneath.
                 */
                state.push_bool(true);
                state.rotate(-(num_results as i32)-1, 1);
                (num_results+1) as c_int
            },
            Err(s) => {
                /* Just push 'false' and the error string */
                state.push_bool(false);
                state.push_string(s.description());
                2
            },
        }
    }

    fn _push_closure(&mut self, f: fn(&mut RumLua)->LuaRet, name: &str) {
        unsafe {
            let stolen = self as *mut RumLua as usize;
            self.state.push_light_userdata(stolen as *mut c_void);
            let fp: *mut Box<Callback> = self.state.new_userdata_typed();
            ptr::write(fp, Box::new(f));
        };
        /* Load the shim generator */
        self.state.push_closure(lua_func!(::RumLua::lua_func_wrapper), 2);
        self.state.raw_geti(lua::REGISTRYINDEX, self.lua_func_shim.value() as lua::Integer);
        self.state.rotate(-2, 1);
        self.state.push(name);
        self.state.pcall(2, 1, 0);
    }
    pub fn register_type<T>(&mut self,
                            mt_name: String,
                            typeinfo: &'static LuaType)
                  where T: Any
    {
        if self.types_str_to_id.contains_key(&mt_name) {
            panic!("Illegally re-registered type. {}", mt_name);
        }

        /* Create the metatable */
        self.state.new_metatable(&mt_name);
        self._push_closure(generic_gc::<T>, "__gc");
        self.state.set_field(-2, "__gc");

        for &(name, f) in typeinfo.methods {
            self._push_closure(f, name);
            self.state.set_field(-2, name);
        }
        // And set the metatable as its own __index
        self.state.set_field(-1, "__index");

        self.types_str_to_id.insert(mt_name.clone(), TypeId::of::<T>());
        self.types_id_to_str.insert(TypeId::of::<T>(), mt_name);
    }

    pub fn register_func_table(&mut self,
                               table_name: &str,
                               funcs: Vec<(&str, Callback)>) {
        self.state.new_table();

        for (name, f) in funcs {
            self._push_closure(f, name);
            self.state.set_field(-2, &name);
        }
        // And save the table to a global
        self.state.set_global(table_name);
    }

    pub fn push<'b, T>(&mut self, objp: &LuaPtr<T>) where T:Any, T:'b {
        let id = TypeId::of::<T>();
        let p: *mut Option<Box<Any>> = self.state.new_userdata_typed();
        let r = Some(Box::new((*objp).clone()) as Box<Any>);
        unsafe { ptr::write(p, r) };
        self.state.set_metatable_from_registry(&self.types_id_to_str[&id]);
    }
    pub fn get<'ret, 'rl, T: Any>(&'rl mut self, index: Index) -> Result<LuaPtr<T>, LuaError>
                   where 'rl: 'ret, T: 'ret
    {
        let id = TypeId::of::<T>();
        if !self.types_id_to_str.contains_key(&id) {
            panic!("Unknown type!");
        }
        let obj = unsafe { self.state.test_userdata_typed::<Box<Any>>(index, &self.types_id_to_str[&id]) };
        match obj {
            Some(bx) => {
                match bx.downcast_ref::<LuaPtr<T>>() {
                    Some(rxf) => Ok(rxf.clone()),
                    _ => panic!("downcast error"),//None,
                }
            },
            _ => Err(Box::new(LError{message: "Error getting object from stack".to_string()})),
        }
    }
}


fn generic_gc<T: Any>(rl: &mut RumLua) -> LuaRet {
    let id = TypeId::of::<T>();
    let typename = &rl.types_id_to_str[&id];
    let obj : Option<&mut Option<Box<Any>>> = unsafe { rl.state.test_userdata_typed(1, typename) };
    match obj {
        None => {
            println!("Error in generic_gc: failed to match item");
        },
        Some(p_ref) => {
            let mut tmp = None;
            unsafe { ptr::swap(p_ref, &mut tmp as *mut Option<Box<Any>>) };
        },
    }
    Ok(0)
}

#[cfg(test)]
mod tests;
