interface SearchBarProps {
  value: string;
  onSearch: (value: string) => void;
}

function SearchBar({ value, onSearch }: SearchBarProps) {
  return (
    <input
      className="sessions-search-input"
      type="text"
      autoFocus
      placeholder="Search sessions..."
      value={value}
      onChange={(event) => onSearch(event.target.value)}
    />
  );
}

export default SearchBar;
