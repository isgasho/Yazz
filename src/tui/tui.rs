use super::{Parameter, ParameterValue, ParamId, FunctionId, SynthParam, ValueRange, MenuItem, FUNCTIONS, OSC_PARAMS, MOD_SOURCES, MOD_TARGETS};
use super::Canvas;
use super::Float;
use super::Label;
use super::{MidiMessage, MessageType};
use super::SoundData;
use super::{UiMessage, SynthMessage};
use super::surface::Surface;
use super::Value;

use crossbeam_channel::{Sender, Receiver};
use log::{info, trace, warn};
use serde::{Serialize, Deserialize};
use termion::{clear, color, cursor};
use termion::color::{Black, White, Red, LightWhite, Reset, Rgb};
use termion::event::Key;

use std::convert::TryInto;
use std::fmt::{self, Debug};
use std::io;
use std::io::{stdout, Write};
use std::num::ParseFloatError;
use std::thread::spawn;
use std::time::{Duration, SystemTime};
use std::cell::RefCell;
use std::rc::Rc;

#[derive(Copy, Clone, PartialEq, Debug)]
enum TuiState {
    Function,
    FunctionIndex,
    Param,
    Value,
}

impl fmt::Display for TuiState {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::Debug::fmt(self, f)
    }
}

fn next(current: TuiState) -> TuiState {
    use TuiState::*;
    match current {
        Function => FunctionIndex,
        FunctionIndex => Param,
        Param => Value,
        Value => Param,
    }
}

fn previous(current: TuiState) -> TuiState {
    use TuiState::*;
    match current {
        Function => Function,
        FunctionIndex => Function,
        Param => FunctionIndex,
        Value => Param,
    }
}

enum ReturnCode {
    KeyConsumed,   // Key has been used, but value is not updated yet
    ValueUpdated,  // Key has been used and value has changed to a valid value
    ValueComplete, // Value has changed and will not be updated again
    KeyMissmatch,  // Key has not been used
    Cancel,        // Cancel current operation and go to previous state
}

#[derive(Debug)]
pub enum ValueHolder {
    Value(ParameterValue),
    SubSelection(&'static mut ParamSelector),
}

#[derive(Debug)]
pub struct ItemSelection {
    pub item_list: &'static [MenuItem], // The MenuItem this item is coming from
    pub item_index: usize,              // Index into the MenuItem list
    pub value: ValueHolder,             // ID or value of the selected item
}

#[derive(Debug)]
pub struct ParamSelector {
    state: TuiState,
    func_selection: ItemSelection,
    param_selection: ItemSelection,
    value: ValueHolder,

    temp_string: String,
    sub_selector: Option<Rc<RefCell<ParamSelector>>>,
}

pub struct Tui {
    // Function selection
    sender: Sender<SynthMessage>,
    ui_receiver: Receiver<UiMessage>,

    // TUI handling
    selector: ParamSelector,
    //sub_selector: ParamSelector,

    // Actual UI
    window: Surface,

    sync_counter: u32,
    idle: Duration, // Accumulated idle times of the engine
    busy: Duration, // Accumulated busy times of the engine
    min_idle: Duration,
    max_busy: Duration,
    canvas: Canvas,
    sound: SoundData, // Sound patch as loaded from disk

    temp_string: String,
}

impl Tui {
    pub fn new(sender: Sender<SynthMessage>, ui_receiver: Receiver<UiMessage>) -> Tui {
        let state = TuiState::Function;
        let sub_func_selection = ItemSelection{item_list: &FUNCTIONS, item_index: 0, value: ValueHolder::Value(ParameterValue::Int(1))};
        let sub_param_selection = ItemSelection{item_list: &OSC_PARAMS, item_index: 0, value: ValueHolder::Value(ParameterValue::Int(1))};
        let temp_string = String::new();
        let sub_selector = ParamSelector{state: TuiState::Function, func_selection: sub_func_selection, param_selection: sub_param_selection, value: ValueHolder::Value(ParameterValue::Int(0)), temp_string: temp_string, sub_selector: Option::None};
        let func_selection = ItemSelection{item_list: &FUNCTIONS, item_index: 0, value: ValueHolder::Value(ParameterValue::Int(1))};
        let param_selection = ItemSelection{item_list: &OSC_PARAMS, item_index: 0, value: ValueHolder::Value(ParameterValue::Int(1))};
        let temp_string = String::new();
        let selector = ParamSelector{state: TuiState::Function, func_selection: func_selection, param_selection: param_selection, value: ValueHolder::Value(ParameterValue::Int(0)), temp_string: temp_string, sub_selector: Option::Some(Rc::new(RefCell::new(sub_selector)))};
        let mut window = Surface::new();
        let temp_string = String::new();
        let sync_counter = 0;
        let idle = Duration::new(0, 0);
        let busy = Duration::new(0, 0);
        let min_idle = Duration::new(10, 0);
        let max_busy = Duration::new(0, 0);
        let canvas = Canvas::new(100, 30);
        let mut sound = SoundData::new();
        sound.init();
        window.set_position(1, 10);
        window.update_all(&sound);

        Tui{sender,
            ui_receiver,
            selector,
            //sub_selector,
            window,
            sync_counter,
            idle,
            busy,
            min_idle,
            max_busy,
            canvas,
            temp_string,
            sound,
        }
    }

    /** Start input handling thread.
     *
     * This thread receives messages from the terminal, the MIDI port, the
     * synth engine and the audio engine.
     */
    pub fn run(to_synth_sender: Sender<SynthMessage>, ui_receiver: Receiver<UiMessage>) -> std::thread::JoinHandle<()> {
        let handler = spawn(move || {
            let mut tui = Tui::new(to_synth_sender, ui_receiver);
            loop {
                let msg = tui.ui_receiver.recv().unwrap();
                match msg {
                    UiMessage::Midi(m)  => tui.handle_midi_event(m),
                    UiMessage::Key(m) => {
                        if Tui::handle_user_input(&mut tui.selector, m, &mut tui.sound) {
                            tui.send_event();
                        }
                    },
                    UiMessage::Param(m) => tui.handle_synth_param(m),
                    UiMessage::SampleBuffer(m, p) => tui.handle_samplebuffer(m, p),
                    UiMessage::EngineSync(idle, busy) => tui.handle_engine_sync(idle, busy),
                };
            }
        });
        handler
    }

    /* MIDI message received */
    fn handle_midi_event(&mut self, m: MidiMessage) {
        match m.get_message_type() {
            MessageType::ControlChg => {
                if m.param == 0x01 { // ModWheel
                    self.handle_control_change(m.value as i64);
                }
            },
            _ => ()
        }
    }

    /* Evaluate the MIDI control change message (ModWheel) */
    fn handle_control_change(&mut self, val: i64) {
        match self.selector.state {
            TuiState::Function => Tui::change_state(&mut self.selector, TuiState::FunctionIndex),
            TuiState::FunctionIndex => (),
            TuiState::Param => Tui::change_state(&mut self.selector, TuiState::Value),
            TuiState::Value => (),
        }
        let item = &mut self.selector.param_selection;
        match item.item_list[item.item_index].val_range {
            ValueRange::IntRange(min, max) => {
                let inc: Float = (max - min) as Float / 127.0;
                let value = min + (val as Float * inc) as i64;
                Tui::update_value(item, ParameterValue::Int(value), &mut self.temp_string);
            }
            ValueRange::FloatRange(min, max) => {
                let inc: Float = (max - min) / 127.0;
                let value = min + val as Float * inc;
                Tui::update_value(item, ParameterValue::Float(value), &mut self.temp_string);
            }
            ValueRange::ChoiceRange(choice_list) => {
                let inc: Float = choice_list.len() as Float / 127.0;
                let value = (val as Float * inc) as i64;
                Tui::update_value(item, ParameterValue::Choice(value as usize), &mut self.temp_string);
            }
            _ => ()
        }
        self.send_event();
    }

    /* Received a queried parameter value from the synth engine. */
    fn handle_synth_param(&mut self, m: SynthParam) {
        let selection = &mut self.selector.param_selection;
        info!("handle_synth_param {} = {:?}", selection.item_list[selection.item_index].item, m);
        Tui::update_value(selection, m.value, &mut self.temp_string);
    }

    /* Received a buffer with samples from the synth engine. */
    fn handle_samplebuffer(&mut self, m: Vec<Float>, p: SynthParam) {
        self.canvas.clear();
        match p.function {
            Parameter::Oscillator => {
                self.canvas.plot(&m, -1.0, 1.0);
            }
            Parameter::Envelope => {
                self.canvas.plot(&m, 0.0, 1.0);
            }
            _ => {}
        }
    }

    /* Received a sync signal from the audio engine.
     *
     * This is used to control timing related actions like drawing the display.
     * The message contains timing data of the audio processing loop.
     */
    fn handle_engine_sync(&mut self, idle: Duration, busy: Duration) {
        self.idle += idle;
        self.busy += busy;
        if idle < self.min_idle {
            self.min_idle = idle;
        }
        if busy > self.max_busy {
            self.max_busy = busy;
        }
        self.sync_counter += 1;
        if self.sync_counter == 10 {
            let display_time = SystemTime::now();
            self.display();
            /*
            println!("{}Display: {:?}\r\nIdle: {:?}\r\nBusy: {:?}\r\nLast: {:?}, {:?}\r\nMin Idle: {:?}, Max Busy: {:?}",
                     cursor::Goto(1, 5), display_time.elapsed().unwrap(), self.idle / 10, self.busy / 10, idle, busy, self.min_idle, self.max_busy);
            self.idle = Duration::new(0, 0);
            self.busy = Duration::new(0, 0);
            */
            self.sync_counter = 0;
            self.query_samplebuffer();
        }
    }

    /* Received a keyboard event from the terminal.
     *
     * Return true if a new value has been read completely, false otherwise.
     */
    fn handle_user_input(mut s: &mut ParamSelector, c: termion::event::Key, sound: &mut SoundData) -> bool {
        let mut key_consumed = false;
        let mut value_change_finished = false;

        while !key_consumed {
            info!("handle_user_input {:?} in state {:?}", c, s.state);
            key_consumed = true;
            let new_state = match s.state {

                // Select the function group to edit (Oscillator, Envelope, ...)
                TuiState::Function => {
                    match Tui::select_item(&mut s.func_selection, c) {
                        ReturnCode::KeyConsumed | ReturnCode::ValueUpdated  => s.state,       // Selection updated
                        ReturnCode::KeyMissmatch | ReturnCode::Cancel       => s.state,       // Ignore key that doesn't match a selection
                        ReturnCode::ValueComplete                           => next(s.state), // Function selected
                    }
                },

                // Select which item in the function group to edit (Oscillator 1, 2, 3, ...)
                TuiState::FunctionIndex => {
                    match Tui::get_value(s, c, sound) {
                        ReturnCode::KeyConsumed   => s.state,           // Key has been used, but value hasn't changed
                        ReturnCode::ValueUpdated  => s.state,           // Selection not complete yet
                        ReturnCode::ValueComplete => {                     // Parameter has been selected
                            s.param_selection.item_list = s.func_selection.item_list[s.func_selection.item_index].next;
                            Tui::select_param(&mut s, sound);
                            next(s.state)
                        },
                        ReturnCode::KeyMissmatch  => s.state,           // Ignore unmatched keys
                        ReturnCode::Cancel        => previous(s.state), // Abort function index selection
                    }
                },

                // Select the parameter of the function to edit (Waveshape, Frequency, ...)
                TuiState::Param => {
                    match Tui::select_item(&mut s.param_selection, c) {
                        ReturnCode::KeyConsumed   => s.state,           // Value has changed, but not complete yet
                        ReturnCode::ValueUpdated  => {                     // Pararmeter selection updated
                            Tui::select_param(&mut s, sound);
                            s.state
                        },
                        ReturnCode::ValueComplete => {                     // Prepare to read the value
                            Tui::select_param(&mut s, sound);
                            next(s.state)
                        },
                        ReturnCode::KeyMissmatch  => s.state,           // Ignore invalid key
                        ReturnCode::Cancel        => previous(s.state), // Cancel parameter selection
                    }
                },

                // Select the parameter value
                TuiState::Value => {
                    // Hack: For modulator settings, we need to pass in a different struct, since
                    // that requires additional submenus.
                    match Tui::get_value(s, c, sound) {
                        ReturnCode::KeyConsumed   => s.state,
                        ReturnCode::ValueUpdated  => { // Value has changed to a valid value, update synth
                            value_change_finished = true;
                            s.state
                        },
                        ReturnCode::ValueComplete => previous(s.state), // Value has changed and will not be updated again
                        ReturnCode::KeyMissmatch  => {
                            // Key can't be used for value, so it probably is the short cut for a
                            // different parameter. Switch to parameter state and try again.
                            key_consumed = false;
                            previous(s.state)
                        },
                        ReturnCode::Cancel => {
                            // Stop updating the value, back to parameter selection
                            previous(s.state)
                        }
                    }
                }
            };
            Tui::change_state(&mut s, new_state);
        }
        value_change_finished
    }

    /* Change the state of the input state machine. */
    fn change_state(selector: &mut ParamSelector, new_state: TuiState) {
        if new_state != selector.state {
            match new_state {
                TuiState::Function => {
                    // We are probably selecting a different function than
                    // before, so we should start the parameter list at the
                    // beginning to avoid out-of-bound errors.
                    selector.param_selection.item_index = 0;
                }
                TuiState::FunctionIndex => {}
                TuiState::Param => {}
                TuiState::Value => {
                    // For modulation parameters, we need to enter a special
                    // sub state
                    let f = &selector.func_selection;
                    if f.item_list[f.item_index].item == Parameter::Modulation {
                    }
                }
            }
            info!("change_state {} -> {}", selector.state, new_state);
            selector.state = new_state;
        }
    }

    /** Gets the current value of the selected parameter from the sound data. */
    fn query_current_value(&self) {
        let function = &self.selector.func_selection.item_list[self.selector.func_selection.item_index];
        let function_id = if let ValueHolder::Value(ParameterValue::Int(x)) = &self.selector.func_selection.value { *x as usize } else { panic!() };
        let parameter = &self.selector.param_selection.item_list[self.selector.param_selection.item_index];
        let param_val = &self.selector.param_selection.value;
        let param = if let ValueHolder::Value(p) = *param_val { p } else { panic!() };
        let param = SynthParam::new(function.item, function_id, parameter.item, param);
        info!("query_current_value {:?}", param);
        self.sender.send(SynthMessage::ParamQuery(param)).unwrap();
    }

    /* Queries a samplebuffer from the synth engine to display.
     *
     * The samplebuffer can contain wave shapes or envelopes.
     */
    fn query_samplebuffer(&self) {
        let buffer = vec!(0.0; 100);
        let function = &self.selector.func_selection.item_list[self.selector.func_selection.item_index];
        let function_id = if let ValueHolder::Value(ParameterValue::Int(x)) = &self.selector.func_selection.value { *x as usize } else { panic!() };
        let parameter = &self.selector.param_selection.item_list[self.selector.param_selection.item_index];
        let param_val = &self.selector.param_selection.value;
        let param = if let ValueHolder::Value(p) = *param_val { p } else { panic!() };
        let param = SynthParam::new(function.item, function_id, parameter.item, param);
        self.sender.send(SynthMessage::SampleBuffer(buffer, param)).unwrap();
    }

    /* Select one of the items of functions or parameters.
     *
     * Called when a new user input is received and we're in the right state for function selection.
     */
    fn select_item(item: &mut ItemSelection, c: termion::event::Key) -> ReturnCode {
        let result = match c {
            Key::Up => {
                if item.item_index < item.item_list.len() - 1 {
                    item.item_index += 1;
                }
                ReturnCode::ValueUpdated
            },
            Key::Down => {
                if item.item_index > 0 {
                    item.item_index -= 1;
                }
                ReturnCode::ValueUpdated
            },
            Key::Left | Key::Backspace => ReturnCode::Cancel,
            Key::Right => ReturnCode::ValueComplete,
            Key::Char('\n') => ReturnCode::ValueComplete,
            Key::Char(c) => {
                for (count, f) in item.item_list.iter().enumerate() {
                    if f.key == c {
                        item.item_index = count;
                        return ReturnCode::ValueComplete;
                    }
                }
                ReturnCode::KeyConsumed
            },
            _ => ReturnCode::KeyConsumed
        };
        info!("select_item {:?}", item.item_list[item.item_index].item);
        result
    }

    /* Construct the value of the selected item.
     *
     * Supports entering the value by
     * - Direct ascii input of the number
     * - Adjusting current value with Up or Down keys
     */
    fn get_value(s: &mut ParamSelector, c: termion::event::Key, sound: &mut SoundData) -> ReturnCode {
        let item: &mut ItemSelection;
        if s.state == TuiState::FunctionIndex {
            item = &mut s.func_selection;
        } else {
            item = &mut s.param_selection;
        }
        info!("get_value {:?}", item.item_list[item.item_index].item);
        match item.item_list[item.item_index].val_range {
            ValueRange::IntRange(min, max) => {
                let mut current = if let ValueHolder::Value(ParameterValue::Int(x)) = item.value { x } else { panic!() };
                let result = match c {
                    Key::Char(x) => {
                        match x {
                            // TODO: This doesn't work well, switch to using the temp_string here as well.
                            '0' | '1' | '2' | '3' | '4' | '5' | '6' | '7' | '8' | '9' => {
                                let y = x as i64 - '0' as i64;
                                let val_digit_added = current * 10 + y;
                                if val_digit_added > max {
                                    current = y; // Can't add another digit, replace current value with new one
                                } else {
                                    current = val_digit_added;
                                }
                                item.value = ValueHolder::Value(ParameterValue::Int(current));
                                if current * 10 > max {
                                    ReturnCode::ValueComplete // Can't add another digit, accept value as final and move on
                                } else {
                                    ReturnCode::KeyConsumed   // Could add more digits, not finished yet
                                }
                            },
                            '\n' => ReturnCode::ValueComplete,
                            _ => ReturnCode::KeyMissmatch,
                        }
                    }
                    Key::Up        => { current += 1; ReturnCode::ValueUpdated },
                    Key::Down      => if current > min { current -= 1; ReturnCode::ValueUpdated } else { ReturnCode::KeyConsumed },
                    Key::Left      => ReturnCode::Cancel,
                    Key::Right     => ReturnCode::ValueComplete,
                    Key::Backspace => ReturnCode::Cancel,
                    _              => ReturnCode::ValueComplete,
                };
                match result {
                    ReturnCode::ValueUpdated | ReturnCode::ValueComplete => Tui::update_value(item, ParameterValue::Int(current), &mut s.temp_string),
                    _ => (),
                }
                result
            },
            ValueRange::FloatRange(min, max) => {
                let mut current = if let ValueHolder::Value(ParameterValue::Float(x)) = item.value { x } else { panic!() };
                let result = match c {
                    Key::Char(x) => {
                        match x {
                            '0' | '1' | '2' | '3' | '4' | '5' | '6' | '7' | '8' | '9' | '.' => {
                                s.temp_string.push(x);
                                let value: Result<Float, ParseFloatError> = s.temp_string.parse();
                                current = if let Ok(x) = value { x } else { current };
                                ReturnCode::KeyConsumed
                            },
                            '\n' => ReturnCode::ValueComplete,
                            _ => ReturnCode::KeyMissmatch,
                        }
                    }
                    Key::Up        => { current += 1.0; ReturnCode::ValueUpdated },
                    Key::Down      => { current -= 1.0; ReturnCode::ValueUpdated },
                    Key::Left      => ReturnCode::Cancel,
                    Key::Right     => ReturnCode::ValueComplete,
                    Key::Backspace => {
                        let len = s.temp_string.len();
                        if len > 0 {
                            s.temp_string.pop();
                            if len >= 1 {
                                let value = s.temp_string.parse();
                                current = if let Ok(x) = value { x } else { current };
                            } else {
                                current = 0.0;
                            }
                        }
                        ReturnCode::KeyConsumed
                    },
                    _ => ReturnCode::KeyMissmatch,
                };
                match result {
                    ReturnCode::ValueUpdated | ReturnCode::ValueComplete => Tui::update_value(item, ParameterValue::Float(current), &mut s.temp_string),
                    _ => (),
                }
                result
            },
            ValueRange::ChoiceRange(choice_list) => {
                let mut current = if let ValueHolder::Value(ParameterValue::Choice(x)) = item.value { x } else { panic!() };
                let result = match c {
                    Key::Up         => {current += 1; ReturnCode::ValueUpdated },
                    Key::Down       => if current > 0 { current -= 1; ReturnCode::ValueUpdated } else { ReturnCode::KeyConsumed },
                    Key::Left | Key::Backspace => ReturnCode::Cancel,
                    Key::Right      => ReturnCode::ValueComplete,
                    Key::Char('\n') => ReturnCode::ValueComplete,
                    _ => ReturnCode::KeyMissmatch,
                };
                match result {
                    ReturnCode::ValueUpdated | ReturnCode::ValueComplete => Tui::update_value(item, ParameterValue::Choice(current), &mut s.temp_string),
                    _ => (),
                }
                result
            },
            ValueRange::ParamRange(choice_list) => {
                // Pass key to sub selector
                match &mut s.sub_selector {
                    Some(sub) => {
                        let value_finished = Tui::handle_user_input(&mut sub.borrow_mut(), c, sound);
                        if value_finished {
                            ReturnCode::ValueComplete
                        } else {
                            ReturnCode::ValueUpdated
                        }
                    },
                    None => panic!(),
                }
            },
            _ => panic!(),
        }
    }

    /* Select the parameter chosen by input. */
    fn select_param(selector: &mut ParamSelector, sound: &SoundData) {
        info!("select_param {:?}", selector.param_selection.item_list[selector.param_selection.item_index].item);
        //let param = item.item_list[item.item_index].item;

        let function = &selector.func_selection.item_list[selector.func_selection.item_index];
        let function_id = if let ValueHolder::Value(ParameterValue::Int(x)) = &selector.func_selection.value { *x as usize } else { panic!() };
        let parameter = &selector.param_selection.item_list[selector.param_selection.item_index];
        let param_val = &selector.param_selection.value;
        let param = if let ValueHolder::Value(p) = *param_val { p } else { panic!() };
        let param = SynthParam::new(function.item, function_id, parameter.item, param);

        // The value in the selected parameter needs to point to the right type.
        // Initialize it with the minimum.
        let value = sound.get_value(&param);
        selector.param_selection.value = match value {
            ParameterValue::Int(_) => ValueHolder::Value(value),
            ParameterValue::Float(_) => ValueHolder::Value(value),
            ParameterValue::Choice(_) => ValueHolder::Value(value),
            ParameterValue::Function(_) => ValueHolder::Value(value),
            ParameterValue::Param(_) => ValueHolder::Value(value),
            ParameterValue::NoValue => panic!(),
        };
        /*
        let val_range = &item.item_list[item.item_index].val_range;
        item.value = match val_range {
            ValueRange::IntRange(min, _) => {
                ValueHolder::Value(ParameterValue::Int(*min))
            }
            ValueRange::FloatRange(min, _) => {
                ValueHolder::Value(ParameterValue::Float(*min))
            }
            ValueRange::ChoiceRange(choice_list) => {
                ValueHolder::Value(ParameterValue::Choice(0))
            }
            ValueRange::ParamRange(choice_list) => {
                /*
                let sub_selector = ParamSelector{state: TuiState::Function, func_selection: sub_func_selection, param_selection: sub_param_selection, value: ValueHolder::Value(ParameterValue::Int(0))};
                match param {
                    Parameter::Source => ValueHolder::SubSelection(),
                    Parameter::Target => ValueHolder::SubSelection(),
                    _ => panic!(),
                }
                */
                ValueHolder::Value(ParameterValue::Choice(0))
            }
            _ => ValueHolder::Value(ParameterValue::NoValue)
        }
        */
    }

    /* Store a new value in the selected parameter. */
    fn update_value(selection: &mut ItemSelection, val: ParameterValue, temp_string: &mut String) {
        info!("update_value item: {:?}, value: {:?}", selection.item_list[selection.item_index].item, val);
        match selection.item_list[selection.item_index].val_range {
            ValueRange::IntRange(min, max) => {
                let mut val = if let ParameterValue::Int(x) = val { x } else { panic!(); };
                if val > max {
                    val = max;
                }
                if val < min {
                    val = min;
                }
                selection.value = ValueHolder::Value(ParameterValue::Int(val.try_into().unwrap()));
            }
            ValueRange::FloatRange(min, max) => {
                let mut val = if let ParameterValue::Float(x) = val { x } else { panic!(); };
                let has_period =  temp_string.contains(".");
                if val > max {
                    val = max;
                }
                if val < min {
                    val = min;
                }
                temp_string.clear();
                temp_string.push_str(val.to_string().as_str());
                if !temp_string.contains(".") && has_period {
                    temp_string.push('.');
                }
                selection.value = ValueHolder::Value(ParameterValue::Float(val));
            }
            ValueRange::ChoiceRange(selection_list) => {
                let mut val = if let ParameterValue::Choice(x) = val { x as usize } else { panic!("{:?}", val); };
                if val >= selection_list.len() {
                    val = selection_list.len() - 1;
                }
                selection.value = ValueHolder::Value(ParameterValue::Choice(val));
            }
            ValueRange::ParamRange(selection_list) => {
                // ParamRange is used for choosing a combination of function-function_id-parameter.
                match val {
                    ParameterValue::Function(x) => {
                        if let ValueHolder::SubSelection(sub) = &mut selection.value {
                            Tui::update_value(&mut sub.func_selection, val, temp_string);
                        } else {
                            panic!();
                        }
                    }
                    ParameterValue::Param(x) => {
                        if let ValueHolder::SubSelection(sub) = &mut selection.value {
                            Tui::update_value(&mut sub.func_selection, val, temp_string);
                            Tui::update_value(&mut sub.param_selection, val, temp_string);
                        } else {
                            panic!();
                        }
                    }
                    _ => panic!(),
                }
            }
            ValueRange::NoRange => {}
        };
    }

    /* Send an updated value to the synth engine. */
    fn send_event(&mut self) {
        // Update sound data
        let function = &self.selector.func_selection.item_list[self.selector.func_selection.item_index];
        let function_id = if let ValueHolder::Value(ParameterValue::Int(x)) = &self.selector.func_selection.value { *x as usize } else { panic!() };
        let parameter = &self.selector.param_selection.item_list[self.selector.param_selection.item_index];
        let param_val = &self.selector.param_selection.value;
        let val = if let ValueHolder::Value(v) = *param_val { v } else { panic!() };
        let param = SynthParam::new(function.item, function_id, parameter.item, val);
        self.sound.set_parameter(&param);

        // Send new value to synth engine
        //info!("send_event {:?}", param);
        self.sender.send(SynthMessage::Param(param)).unwrap();

        // Update UI
        let param_id = ParamId{function: function.item, function_id: function_id, parameter: parameter.item};
        let value = match val {
            ParameterValue::Float(v) => Value::Float(v.into()),
            ParameterValue::Int(v) => Value::Int(v),
            ParameterValue::Choice(v) => Value::Int(v.try_into().unwrap()),
            _ => return
        };
        self.window.update_value(&param_id, value);
    }

    /* ====================================================================== */

    /** Display the UI. */
    fn display(&self) {
        print!("{}{}", clear::All, cursor::Goto(1, 1));
        Tui::display_selector(&self.selector);

        self.window.draw();

        io::stdout().flush().ok();
    }

    fn display_selector(s: &ParamSelector) {
        let mut display_state = TuiState::Function;
        let mut x_pos: u16 = 1;
        loop {
            match display_state {
                TuiState::Function => {
                    Tui::display_function(s, s.state == TuiState::Function);
                }
                TuiState::FunctionIndex => {
                    Tui::display_function_index(s, s.state == TuiState::FunctionIndex);
                    x_pos = 12;
                }
                TuiState::Param => {
                    Tui::display_param(s, s.state == TuiState::Param);
                    x_pos = 14;
                }
                TuiState::Value => {
                        Tui::display_value(s, s.state == TuiState::Value);
                        x_pos = 23;
                }
            }
            if display_state == s.state {
                break;
            }
            display_state = next(display_state);
        }
        Tui::display_options(s, x_pos);
        //self.display_samplebuff();
    }

    fn display_function(s: &ParamSelector, selected: bool) {
        let func = &s.func_selection;
        if selected {
            print!("{}{}", color::Bg(LightWhite), color::Fg(Black));
        } else {
            print!("{}{}", color::Bg(Rgb(255, 255, 255)), color::Fg(Black));
        }
        print!("{}", func.item_list[func.item_index].item);
        if selected {
            print!("{}{}", color::Bg(Rgb(255, 255, 255)), color::Fg(Black));
        }
    }

    fn display_function_index(s: &ParamSelector, selected: bool) {
        let func = &s.func_selection;
        if selected {
            print!("{}{}", color::Bg(LightWhite), color::Fg(Black));
        }
        let function_id = if let ValueHolder::Value(ParameterValue::Int(x)) = &func.value { *x as usize } else { panic!() };
        print!(" {}", function_id);
        if selected {
            print!("{}{}", color::Bg(Rgb(255, 255, 255)), color::Fg(Black));
        }
    }

    fn display_param(s: &ParamSelector, selected: bool) {
        let param = &s.param_selection;
        if selected {
            print!("{}{}", color::Bg(LightWhite), color::Fg(Black));
        }
        print!(" {}", param.item_list[param.item_index].item);
        if selected {
            print!("{}{}", color::Bg(Rgb(255, 255, 255)), color::Fg(Black));
        }
    }

    fn display_value(s: &ParamSelector, selected: bool) {
        let param = &s.param_selection;
        if selected {
            print!("{}{}", color::Bg(LightWhite), color::Fg(Black));
        }
        let value = if let ValueHolder::Value(x) = param.value { x } else { panic!() };
        match value {
            ParameterValue::Int(x) => print!(" {}", x),
            ParameterValue::Float(x) => print!(" {}", x),
            ParameterValue::Choice(x) => {
                let item = &param.item_list[param.item_index];
                let range = &item.val_range;
                let selection = if let ValueRange::ChoiceRange(list) = range { list } else { panic!() };
                let item = selection[x].item;
                print!(" {}", item);
            },
            ParameterValue::Function(x) => {
                match &s.sub_selector {
                    Some(sub) => Tui::display_selector(&sub.borrow()),
                    None => panic!(),
                }
            },
            ParameterValue::Param(x) => {
                match &s.sub_selector {
                    Some(sub) => Tui::display_selector(&sub.borrow()),
                    None => panic!(),
                }
            },
            _ => ()
        }
        if selected {
            print!("{}{}", color::Bg(Rgb(255, 255, 255)), color::Fg(Black));
        }
    }

    fn display_options(s: &ParamSelector, x_pos: u16) {
        if s.state == TuiState::Function {
            let mut y_item = 2;
            let list = s.func_selection.item_list;
            for item in list.iter() {
                print!("{}{} - {}", cursor::Goto(x_pos, y_item), item.key, item.item);
                y_item += 1;
            }
        }
        if s.state == TuiState::FunctionIndex {
            let item = &s.func_selection.item_list[s.func_selection.item_index];
            let (min, max) = if let ValueRange::IntRange(min, max) = item.val_range { (min, max) } else { panic!() };
            print!("{}{} - {}", cursor::Goto(x_pos, 2), min, max);
        }
        if s.state == TuiState::Param {
            let mut y_item = 2;
            let list = s.param_selection.item_list;
            for item in list.iter() {
                print!("{}{} - {}", cursor::Goto(x_pos, y_item), item.key, item.item);
                y_item += 1;
            }
        }
        if s.state == TuiState::Value {
            let range = &s.param_selection.item_list[s.param_selection.item_index].val_range;
            match range {
                ValueRange::IntRange(min, max) => print!("{}{} - {}", cursor::Goto(x_pos, 2), min, max),
                ValueRange::FloatRange(min, max) => print!("{}{} - {}", cursor::Goto(x_pos, 2), min, max),
                ValueRange::ChoiceRange(list) => print!("{}1 - {}", cursor::Goto(x_pos, 2), list.len()),
                ValueRange::ParamRange(list) => (),
                ValueRange::NoRange => ()
            }
        }
    }

    fn display_samplebuff(&self) {
        print!("{}{}", color::Bg(Black), color::Fg(White));
        self.canvas.render(1, 10);
        print!("{}{}", color::Bg(Rgb(255, 255, 255)), color::Fg(Black));
    }
}