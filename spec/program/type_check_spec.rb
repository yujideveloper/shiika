require 'spec_helper'

describe "Type check" do
  SkTypeError = Shiika::Program::SkTypeError

  def type!(src)
    ast = Shiika::Parser.new.parse(src)
    prog = ast.to_program
    prog.add_type!
  end

  context 'method call' do
    it 'arity' do
      src = <<~EOD
         class A
           def self.foo(x: Int, y: Int) -> Void
           end
         end
         A.foo(1)
      EOD
      expect{ type!(src) }.to raise_error(SkTypeError)
    end

    it 'argument type' do
      src = <<~EOD
         class A
           def self.foo(x: Int) -> Void
           end
         end
         A.foo(true)
      EOD
      expect{ type!(src) }.to raise_error(SkTypeError)
    end
  end

  context 'variable assignment'

  context 'generics' do
    it 'number of type arguments' do
      src = <<~EOD
         class A<S, T>
         end
         A<Int>
      EOD
      expect{ type!(src) }.to raise_error(SkTypeError)
    end

    it 'type of initializer' do
      src = <<~EOD
         class A<T>
           def initialize(x: T); end
         end
         A<Int>.new(true)
      EOD
      expect{ type!(src) }.to raise_error(SkTypeError)
    end

    it 'type of instance method' do
      src = <<~EOD
         class A<T>
           def foo(x: T) -> Void; end
         end
         A<Int>.new.foo(true)
      EOD
      expect{ type!(src) }.to raise_error(SkTypeError)
    end

    it 'type of instance variable' do
      src = <<~EOD
         class A<T>
           def initialize(@a: T)
             @a = 1
           end
         end
         A<Bool>.new
      EOD
      expect{ type!(src) }.to raise_error(SkTypeError)
    end
  end
end
