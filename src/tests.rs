use ::{RumLua, LuaType, LuaRet, LuaPtr};
use lua;
use std::rc::Rc;
use std::cell::RefCell;
use std::error;
use std::fmt::{Display, Formatter};
use std::fmt;

#[derive(Debug)]
struct TestDrop {
    dropcount: Rc<RefCell<u32>>,
}

impl Drop for TestDrop {
    fn drop(&mut self) {
        let x:u32 = *self.dropcount.borrow();
        *self.dropcount.borrow_mut() = x+1;
        println!("Dropping TestDrop");
    }
}

#[test]
    #[allow(unused_variables)]
fn lua_start() {
    let rlua = RumLua::new();
}

static EMPTY_METHODS: LuaType = LuaType{ methods: &[], };

#[test]
fn lua_register() {
    {
        let mut rlua = RumLua::new();
        rlua.register_type::<TestDrop>("testdrop".to_string(), &EMPTY_METHODS);
    }
}
#[test]
fn lua_drop() {
    let dropcount = Rc::new(RefCell::new(0u32));
    {
        let mut rlua = RumLua::new();
        rlua.register_type::<TestDrop>("TestDrop".to_string(), &EMPTY_METHODS);
        let ts = TestDrop{ dropcount: dropcount.clone() };
        rlua.push(&LuaPtr::new(ts));
//        rlua.state.set_metatable_from_registry("TestDrop");
        TestDrop{ dropcount: dropcount.clone() };
    }
    /* Lua state should be destroyed, and the dropcount incremented. */
    assert_eq!(*dropcount.borrow(), 2u32);
}

#[derive(Debug)]
struct TestMeth {
    data: String,
}
impl TestMeth {
    pub fn get(&self) -> String {
        self.data.clone()
    }
    pub fn set(&mut self, s: &str) {
        self.data = s.to_string();
    }
}

static SOME_METHODS: LuaType = LuaType{
    methods: &[
        ("get", test_method_get),
        ("set", test_method_set),
    ], };

fn test_method_get(rl: &mut RumLua) -> LuaRet {
    let tobj = try!(rl.get::<TestMeth>(1));
    rl.state.push(tobj.borrow().get());
    Ok(1)
}

fn test_method_set(rl: &mut RumLua) -> LuaRet {
    let mut tobj = try!(rl.get::<TestMeth>(1));
    tobj.borrow_mut().set(rl.state.to_str(2).unwrap());
    Ok(0)
}

#[test]
fn lua_meth1() {
    let mut rlua = RumLua::new();
    rlua.register_type::<TestMeth>("TestMeth".to_string(), &SOME_METHODS);

    rlua.push(&LuaPtr::new(TestMeth{data: "foo".to_string()}));
    rlua.state.set_global("testvar");
    rlua.do_string("
        testvar:set(testvar:get() .. 'bar')
    ");
    rlua.state.get_global("testvar");
    let tvar = rlua.get::<TestMeth>(1).unwrap();
    assert_eq!(tvar.borrow().data, "foobar");
}

#[derive(Debug)]
struct TestError(String);
impl error::Error  for TestError {
    fn description(&self) -> &str {
        &self.0
    }
    fn cause(&self) -> Option<&error::Error> {
        None
    }
}

impl Display for TestError {
    fn fmt(&self, f: &mut Formatter) -> Result<(), fmt::Error> {
        write!(f, "TestError({})", self.0).unwrap();
        Ok(())
    }
}

fn test_fail(_: &mut RumLua) -> LuaRet {
    Err(Box::new(TestError("foo".to_string())))
}
fn test_seven(rl: &mut RumLua) -> LuaRet {
    rl.state.push(7);
    Ok(1)
}

#[test]
fn lua_errors() {
    let mut rlua = RumLua::new();
    rlua.register_func_table("funcs", vec![
        ("fail", test_fail),
        ("ret7", test_seven),
    ]);
    rlua.do_string(r#"
        local x = funcs.ret7()
        local ok, err = pcall(funcs.fail)
        result1 = "ret7 returned "..x
        result2 = "fail returned ["..tostring(ok).."], ["..tostring(err).."]"
    "#);
    assert_eq!(rlua.state.get_global("result1"), lua::Type::String);
    assert_eq!(rlua.state.to_str(-1).unwrap(), "ret7 returned 7");
    assert_eq!(rlua.state.get_global("result2"), lua::Type::String);
    assert_eq!(rlua.state.to_str(-1).unwrap(), "fail returned [false], [Calling fail:\nfoo]");
}
