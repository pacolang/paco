<br>
<img width="330" src="https://i.imgur.com/WmpRtQP.png" align="left"/>

### 👩🏼‍💻 A simple compiled programming language.
The language is wrote in **Go** and the target language is **C**. The built-in library is wrote in **C** too.
This project is a prototype ⚠️ Please consider it.

<br>

## Example
### Code example
```paco
- returns a random number between 0 and 6
fn rollDice() int
    random|randInt(6)
end

- gets the user entry
console|println("Enter your name")
name = console|getStringEntry()

- uses pipe character to use multiple time built-in functions coming
- from the same package
console
    |print("Hello, ")
    |println(*name)
    |println("Please enter a number")

- asks the user to enter a number and prints the number given
number = console|getIntEntry()
stdio|printf("You've entered: %d\n" *number)

- check if the given number is in the 0-6 range
if *number < 0 or *number > 6
    console|println("the number must be contained between 0 and 6")
- roll the dice and check if the given number is the same as the computer
else
    computerNumber = rollDice()
    if *number == *computerNumber
        console|println("you won! same number as the computer!")
    else
        stdio|printf("you lost! the computer's number was %d" *computerNumber)
    end
end
```

### Module file
```paco
mod "console"

fn println(string)
fn print(string)
fn getStringEntry() string
fn getIntEntry() int
```

<p align="center">
     <img width="100" src="https://i.imgur.com/RleFr3v.png"/><br>
     Made by <a href="https://github.com/hugolgst">Hugo Lageneste</a>
</p>
